# Hosting WordCraft for the web

`wordcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `wordcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `wordcraft-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `wordcraft-web-<hash>_bg.wasm` | The app, about 13 MB, or 5 MB with compression |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. Upload the folder's contents anywhere that serves static files.

## Any path works

All URLs in `index.html` are relative (`public_url = "./"` in `apps/wordcraft-web/Trunk.toml`),
so the site works at a domain root (`https://example.com/`), under a prefix
(`https://example.com/tools/wordcraft/`) and from a CDN bucket. The asset names carry a content
hash, so they can be cached forever. Only `index.html` needs revalidation.

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`. Browsers refuse to stream-compile it under
  any other type, and the app then loads slowly or not at all. Serve `.js` as `text/javascript`.
  Most hosts already do both. For nginx, check that `mime.types` has `application/wasm wasm;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`. That takes the
  download from about 13 MB to about 5 MB. You can also precompress (`brotli -k *.wasm`) and let
  the server send `Content-Encoding: br`.
- **Caching:** `Cache-Control: public, max-age=31536000, immutable` on the hashed `.wasm` and
  `.js` files, and `no-cache` on `index.html`.
- **HTTPS:** WebGPU (and the clipboard) only work in a secure context, which means `https://`
  or `http://localhost`. Over plain HTTP elsewhere, the app falls back to WebGL2.
- **No special isolation headers:** WordCraft doesn't use `SharedArrayBuffer`, so it doesn't
  need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`. If your site already sends
  COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`
  when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /wordcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; }
    gzip on;
    gzip_types application/wasm text/javascript text/html;
    location ~* \.(wasm|js)$ { add_header Cache-Control "public, max-age=31536000, immutable"; }
    location ~* index\.html$ { add_header Cache-Control "no-cache"; }
}
```

Local test: `python3 -m http.server 8765` inside the folder, then open http://localhost:8765/.

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/wordcraft/"
  title="WordCraft image editor"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Cross-origin embeds** work. Preferences are kept in the iframe's `localStorage`. Browsers
  that partition or block third-party storage may forget them between visits, and the app
  then starts with defaults.
- **Sandboxed iframes** need at least
  `sandbox="allow-scripts allow-same-origin allow-downloads allow-popups"`. Without
  `allow-same-origin` there's no storage. Without `allow-downloads`, Save and Export (browser
  downloads) are blocked.
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Renderer selection and fallback flags

WordCraft renders with wgpu. It uses **WebGPU** when the browser has it and falls back to
**WebGL2** on its own. URL query flags override this, and they work on the iframe `src` too:

| Flag | Effect |
|---|---|
| *(none)* | WebGPU if available, otherwise WebGL2 |
| `?webgl` | Force the WebGL2 backend (useful when a WebGPU driver misbehaves) |

For example: `<iframe src="https://example.com/wordcraft/?webgl" ...>`.

A browser with neither WebGPU nor WebGL2 gets a message in place of the app.

## Embedded document recovery (WORD-06)

Embedded in the ERP page (`?embed=erpcons&parent=…`, the shell's `window.erpEmbed` bridge) or with
`?host=parent`, the editor keeps its own recovery copy of the host's document in IndexedDB database
`erpcons-embed-drafts`, store `documents` (one record per document: `{key, name, format, bytes}`),
apart from the shell's standalone drafts. Contract with the parent (protocol v1 messages,
`{ns: "erp-office", v: 1, type, …}`, checked for the parent's origin and window):

- Once it listens, the editor posts `{type: "drafts-ready"}` to the parent's origin.
- The parent answers (and after every iframe reload) with
  `{"ns":"erp-office","v":1,"type":"init","docId":"drive-item-stable-id","name":"Document.docx","drafts":"local"}`.
  All fields but `type` are optional; they may also ride on the usual `open` message. An `open`
  without `docId`/`drafts` right after `init` keeps the identity `init` set.
- `docId` identifies the document: keep it stable across reloads and renames, different for
  another document. Without it the key is the file name. Either is hashed with the parent origin
  (SHA-256), so other parents never see the record.
- `drafts: "host"` means the host keeps the drafts: nothing is stored or restored locally. Any
  other value, or none, keeps local recovery on.
- A shell that sends no `init` gets recovery keyed by the file name it delivered
  (`erpEmbed.originalName()`). Send `init` before the user can edit: a document changed before
  its identity is known is first stored at its next edit.

Only the host's document is stored: the first unsaved document named like it (case and
extension aside), or the restored copy. On reload the stored copy opens as an unsaved document
under its own name next to the one the parent sent; saving under that name saves to the ERP as
usual, and saving or closing it deletes the record. The shell's `erpDrafts.put/remove` stub keeps
receiving the calls that drive the `dirty` message. IndexedDB availability and quota stay with the
browser; a failed write is logged and retried at the next draft.

## Image clipboard limits (WORD-05)

Clipboard HTML carries pictures as `data:` URIs up to **4 MiB decoded each** and **16 MiB decoded
in total**, in at most 32 MiB of HTML (including Văn bản's private marker); pictures over a limit
are written as an `<img>` without a source, marked `data-erpcons-image="too-large"`. Pasting never
fetches http(s) or file URLs. A picture that cannot be kept (URL only, over a limit, more than
16 384 pixels a side, or not PNG/JPEG/GIF/WebP/BMP) becomes its alt text or a visible `▧`, and a
notice counts the pictures not pasted and why. Copying exactly one picture also offers it as a
PNG bitmap: the desktop app puts the bitmap on the system clipboard and keeps the full copy in
process (pasting back into Văn bản keeps size, wrapping and alt text); the browser writes HTML,
text and PNG in one `ClipboardItem` where it supports `image/png`, else HTML and text.
