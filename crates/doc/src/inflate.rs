//! Bounded inflation of zip entries from files (DOCX and its templates, ODT, spreadsheets read as
//! recipient lists): the one place the decompression limits are chosen, per build target, and
//! enforced. Files arrive from outside (mail, chat, ERP attachments), so a package that would
//! inflate past these is refused before it can exhaust memory. Plain `std::io::Read`, so the
//! format crates share it without this crate depending on a zip library.

use std::io::Read;

/// The whole interface message for a refused file; refusals carry it so the interface can
/// recognise and translate them.
pub const TOO_LARGE: &str = "The file is too large or abnormally compressed, so it can't be opened.";

/// How far one package may inflate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest inflated entry, in bytes.
    pub entry: u64,
    /// Largest inflated total read from one package, in bytes.
    pub total: u64,
    /// Most entries in the zip directory.
    pub entries: usize,
    /// Largest inflated/compressed ratio, checked once an entry passes [`Limits::ratio_floor`].
    pub ratio: u64,
    /// Inflated size below which the ratio is not checked: small parts of a few repeated
    /// elements legitimately compress far beyond any useful ratio.
    pub ratio_floor: u64,
}

const MIB: u64 = 1024 * 1024;

// Measured on generated parts: text and tables deflate 8–25:1, mail-merge-like documents ~55:1
// and 300 identical table pages 184:1, while a deflate bomb runs at ~1000:1 (deflate's ceiling is
// 1032:1). 200:1 leaves legitimate files room; a bomb tuned to stay just under it still meets the
// size caps. Letting an entry reach 4 MiB before the ratio applies costs nothing and keeps small
// repetitive parts clear.
const RATIO: u64 = 200;
const RATIO_FLOOR: u64 = 4 * MIB;

/// Desktop: the limits WordCraft has always had.
pub const NATIVE: Limits = Limits { entry: 256 * MIB, total: 512 * MIB, entries: 20_000, ratio: RATIO, ratio_floor: RATIO_FLOOR };

/// Browser tab: wasm32 has 4 GiB of address space at most and a tab often gets less, and every
/// part of a package stays inflated while the document, its layout and the file itself are in
/// memory too. 256 MiB in all keeps that to an eighth of a 2 GiB heap.
pub const WEB: Limits = Limits { entry: 128 * MIB, total: 256 * MIB, entries: 20_000, ratio: RATIO, ratio_floor: RATIO_FLOOR };

/// The limits for a target: [`WEB`] for wasm32, [`NATIVE`] otherwise.
pub const fn for_target(wasm32: bool) -> Limits {
    if wasm32 { WEB } else { NATIVE }
}

/// The limits of this build.
pub const LIMITS: Limits = for_target(cfg!(target_arch = "wasm32"));

/// Why an entry wasn't read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InflateError {
    /// A package limit: the whole file is refused. Shows as [`TOO_LARGE`] and the detail.
    Refused(String),
    /// Over the caller's own limit for this entry, by what the header declares or inflates.
    TooBig(String),
    /// The entry is damaged.
    Damaged(String),
}

impl std::fmt::Display for InflateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InflateError::Refused(d) => write!(f, "{TOO_LARGE} ({d})"),
            InflateError::TooBig(d) | InflateError::Damaged(d) => f.write_str(d),
        }
    }
}

/// Inflation is read in steps this large, so every limit is checked on what actually came out
/// rather than on the sizes the zip headers declare.
const CHUNK: usize = 64 * 1024;
/// Most capacity reserved up front from a declared size, which a hostile file can inflate.
const MAX_PREALLOC: u64 = 16 * MIB;

/// The inflation budget of one package.
#[derive(Clone, Debug)]
pub struct Inflater {
    limits: Limits,
    total: u64,
    /// Sticky: a reader that skips an unreadable optional part must not turn a bomb into a
    /// partly opened document.
    refused: Option<String>,
}

impl Inflater {
    pub fn new(limits: Limits) -> Inflater {
        Inflater { limits, total: 0, refused: None }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Why the package was refused, once an entry broke a package limit.
    pub fn refusal(&self) -> Option<InflateError> {
        self.refused.clone().map(InflateError::Refused)
    }

    /// Refuses a zip directory with more than the limit's entries.
    pub fn check_entries(&mut self, entries: usize) -> Result<(), InflateError> {
        if entries > self.limits.entries {
            return Err(self.refuse(format!("{entries} zip entries, more than {}", self.limits.entries)));
        }
        Ok(())
    }

    fn refuse(&mut self, reason: String) -> InflateError {
        InflateError::Refused(self.refused.get_or_insert(reason).clone())
    }

    /// Inflates entry `name` from `reader`, which the zip library bounds to the `compressed`
    /// bytes of the archive. `declared` is the header's inflated size, trusted only to fail early.
    /// Going over `maximum` is the caller's to handle; going over a package limit, by header or
    /// by inflation, refuses the whole package (see [`Inflater::refusal`]).
    pub fn read(&mut self, name: &str, mut reader: impl Read, declared: u64, compressed: u64, maximum: u64) -> Result<Vec<u8>, InflateError> {
        if let Some(e) = self.refusal() {
            return Err(e);
        }
        let limits = self.limits;
        let maximum = maximum.min(limits.entry);
        let remaining = limits.total.saturating_sub(self.total);
        let limit = maximum.min(remaining);
        // Skipping the entry instead would leave a required part missing, and the file would
        // then be reported as corrupt rather than too large.
        if declared > limit {
            return Err(self.over(format!("{name} declares {declared} bytes"), declared, remaining, maximum));
        }
        let compressed = compressed.max(1);
        let ratio_exceeded = |inflated: u64| inflated > limits.ratio_floor && inflated > compressed.saturating_mul(limits.ratio);
        let mut out: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; CHUNK];
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(InflateError::Damaged(format!("{name}: {e}"))),
            };
            let inflated = (out.len() as u64).saturating_add(n as u64);
            if ratio_exceeded(inflated) {
                return Err(self.refuse(format!("{name} is compressed more than {}:1", limits.ratio)));
            }
            if inflated > limit {
                return Err(self.over(format!("{name} inflates to more than {limit} bytes"), inflated, remaining, maximum));
            }
            if reserve(&mut out, n, declared, limit).is_err() {
                return Err(self.refuse(format!("not enough memory to inflate {name}")));
            }
            out.extend_from_slice(chunk.get(..n).unwrap_or_default());
        }
        self.total = self.total.saturating_add(out.len() as u64);
        Ok(out)
    }

    /// An entry of `size` bytes, over its limit: past a package limit it refuses the package,
    /// past only the caller's smaller `maximum` it fails just this entry.
    fn over(&mut self, what: String, size: u64, remaining: u64, maximum: u64) -> InflateError {
        if size > remaining {
            self.refuse(format!("{what}: the package may inflate to {} bytes at most", self.limits.total))
        } else if size > self.limits.entry {
            self.refuse(format!("{what}: an entry may inflate to {} bytes at most", self.limits.entry))
        } else {
            InflateError::TooBig(format!("{what}, more than {maximum}"))
        }
    }
}

/// Makes room for `n` more bytes. Capacity doubles but never past `limit` (where the read stops
/// anyway), and an allocation failure is an error rather than an abort of the whole tab.
fn reserve(out: &mut Vec<u8>, n: usize, declared: u64, limit: u64) -> Result<(), std::collections::TryReserveError> {
    let needed = out.len().saturating_add(n);
    if needed <= out.capacity() {
        return Ok(());
    }
    let first = usize::try_from(declared.min(MAX_PREALLOC)).unwrap_or(usize::MAX);
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let target = out.capacity().saturating_mul(2).max(first).min(limit).max(needed);
    out.try_reserve_exact(target.saturating_sub(out.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Limits = Limits { entry: 3 * MIB, total: 5 * MIB, entries: 4, ratio: RATIO, ratio_floor: MIB };

    /// Bytes that barely compress (xorshift), as a stand-in for any inflated stream.
    fn noise(len: usize) -> Vec<u8> {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn the_web_build_gets_the_smaller_limits() {
        assert_eq!(for_target(true), WEB);
        assert_eq!(for_target(false), NATIVE);
        assert_eq!(LIMITS, for_target(cfg!(target_arch = "wasm32")));
        const { assert!(WEB.entry < NATIVE.entry && WEB.total < NATIVE.total) };
        for l in [WEB, NATIVE] {
            assert!(l.entry <= l.total && l.ratio_floor < l.entry && l.ratio >= 100);
        }
    }

    #[test]
    fn a_high_ratio_refuses_the_package_while_streaming() {
        let mut inf = Inflater::new(SMALL);
        let zeros = vec![0u8; 2 * MIB as usize];
        let e = inf.read("bomb.xml", &zeros[..], 10, 1000, u64::MAX).unwrap_err();
        assert!(matches!(&e, InflateError::Refused(m) if m.contains("200:1")));
        assert!(e.to_string().starts_with(TOO_LARGE));
        assert!(matches!(inf.read("other.xml", &b"<a/>"[..], 4, 4, u64::MAX), Err(InflateError::Refused(_))), "sticky");
    }

    #[test]
    fn a_declared_small_entry_is_cut_off_at_the_entry_cap() {
        let data = noise(4 * MIB as usize);
        let mut inf = Inflater::new(SMALL);
        let e = inf.read("big.bin", &data[..], 100, data.len() as u64, u64::MAX).unwrap_err();
        assert!(matches!(e, InflateError::Refused(m) if m.contains("inflates to more than")));
    }

    #[test]
    fn the_total_and_the_entry_count_are_capped() {
        let data = noise(2 * MIB as usize + 1);
        let mut inf = Inflater::new(SMALL);
        assert_eq!(inf.read("a", &data[..], 0, data.len() as u64, u64::MAX).unwrap().len(), data.len());
        assert_eq!(inf.read("b", &data[..], 0, data.len() as u64, u64::MAX).unwrap().len(), data.len());
        assert!(matches!(inf.read("c", &data[..], 0, data.len() as u64, u64::MAX), Err(InflateError::Refused(m)) if m.contains("package")));
        assert!(Inflater::new(SMALL).check_entries(4).is_ok());
        assert!(matches!(Inflater::new(SMALL).check_entries(5), Err(InflateError::Refused(_))));
    }

    #[test]
    fn a_caller_limit_fails_only_that_entry() {
        let data = noise(MIB as usize);
        let mut inf = Inflater::new(SMALL);
        assert!(matches!(inf.read("meta.xml", &data[..], 0, data.len() as u64, 1024), Err(InflateError::TooBig(_))));
        assert!(matches!(inf.read("meta.xml", &data[..], data.len() as u64, data.len() as u64, 1024), Err(InflateError::TooBig(_))));
        assert!(inf.refusal().is_none());
        assert_eq!(inf.read("meta.xml", &data[..], data.len() as u64, data.len() as u64, u64::MAX).unwrap(), data);
        // Under the ratio floor, any ratio passes.
        assert_eq!(inf.read("small.xml", &vec![b' '; 900_000][..], 0, 1, u64::MAX).unwrap().len(), 900_000);
    }

    #[test]
    fn a_size_declared_over_a_package_limit_refuses_without_inflating() {
        let mut inf = Inflater::new(SMALL);
        let mut untouched: &[u8] = &[1, 2, 3];
        let e = inf.read("huge.xml", &mut untouched, 1 << 40, 10, u64::MAX).unwrap_err();
        assert!(matches!(&e, InflateError::Refused(m) if m.contains("declares")), "{e}");
        assert_eq!(untouched.len(), 3, "nothing was inflated");
        assert!(inf.refusal().is_some(), "sticky");
        // A caller's smaller limit doesn't make it a part that can just be skipped.
        let e = Inflater::new(SMALL).read("[Content_Types].xml", &[][..], 4 * MIB, 10, 1024).unwrap_err();
        assert!(matches!(&e, InflateError::Refused(m) if m.contains("an entry")), "{e}");
        let mut inf = Inflater::new(SMALL);
        inf.read("a", &noise(5 * MIB as usize / 2)[..], 0, 1 << 30, u64::MAX).unwrap();
        let e = inf.read("b", &b"<a/>"[..], 3 * MIB, 10, u64::MAX).unwrap_err();
        assert!(matches!(&e, InflateError::Refused(m) if m.contains("package")), "{e}");
    }
}
