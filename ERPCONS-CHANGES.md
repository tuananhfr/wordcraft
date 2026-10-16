# ERPCons changes

This branch (`erpcons`) is the ERPCons Văn phòng fork of [WordCraft](https://github.com/storytold/wordcraft), based on WordCraft by the ArtCraft team, licensed MIT OR Apache-2.0 (see `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`). Changes from upstream `main` (Apache-2.0 §4b):

- Removed the ArtCraft logos and marks (`docs/brand/`) and every link to the ArtCraft Discord and getartcraft.com, as the ArtCraft brand terms require for forks: About dialog, title-bar Discord button and its Options switch, Backstage and ribbon Discord entries, the `ui.discord` command, README.
- About now reads "ERPCons Văn phòng — dựa trên WordCraft của nhóm ArtCraft. MIT OR Apache-2.0." and links this fork's source.
- Sample document, LaTeX export header and MCP server info point at this fork or office.erpcons.vn instead of getartcraft.com.
- Dropped the translations of the removed strings and the word "artcraft" from the extra spelling list.
- Vietnamese Telex/VNI (and other input methods that rewrite what they typed) no longer garble text on the web: the canvas handles the browser's `DeleteSurrounding` IME event, deleting the taken-back characters around the caret (within its paragraph) together with the replacement in one Typing undo step. `text.insert` and `equation.type` take optional `deleteBefore` / `deleteAfter` counts for this.
- Web "Save changes?" → Save no longer loses the document when the browser's save picker is cancelled: `Services::download` reports a picker save as `Download::Pending`, the held-back New/Open/Close runs only once the file is written, and a cancelled or failed picker save marks the document it saved (not whichever is open) unsaved again. Anchor downloads (browsers without the picker) behave as before.
