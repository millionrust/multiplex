# Multiplex browser terminals

This is the browser view embedded in Multiplex. The running application serves it entirely through Rust: **no Node, Bun, npm, or separate web process is needed**.

In Multiplex, open **Remote Devices → This computer → Browser terminal access → On**. The same controls are available under **Settings → Remote Devices**. Click Open for a browser on this computer. For a phone or another computer, copy one of the displayed LAN/Tailscale links. Copy the access code separately, and paste it on the viewer's sign-in page.

The server starts disabled. Disabling it, or closing its owning Multiplex window, shuts down its listeners and browser connections. Enabling it again rotates the access code and browser cookie. The preferred port is 7417; if another instance owns that port, Multiplex selects an available port. Only loopback and private interfaces are bound. Interfaces are discovered on enable; after a network change, toggle Off/On to refresh links. Tailscale links use the device's private Tailscale IP, so no MagicDNS configuration is necessary.

Browser access uses HTTP on those private interfaces, an access code, a HttpOnly/SameSite=Strict cookie, exact Host/Origin checks, login rate limits, bounded request bodies, and bounded terminal operations. Use a trusted LAN/Tailscale route or an encrypted tunnel; this does not provide public Internet hosting or built-in HTTPS. Browser permissions are separate from paired-phone permissions: anyone given the browser access code can view, input, rename, resize, pin, and kill this computer's CLI terminals. Turning the server off revokes that access.

## View and controls

- Sessions lists only live `multiplex-cli shell` sessions, including local app terminals. No tmux executable or tmux socket is used.
- Filter by title, shell, or folder. Grid shows previews; focus view shows the terminal. Selection pauses rendering while you select and copy.
- Input/read-only toggle, keyboard shortcuts, direct paste up to 1 MiB, and bracketed-paste handling. Mobile offers Esc, Tab, Shift+Tab, Control+C, arrows, and Enter.
- Pins live beside the CLI session record and are shared across browsers. Double-click a sidebar title to rename; Enter or clicking outside saves, Escape cancels, and an empty name restores its default. Renames also appear in desktop/mobile session lists.
- Drag a sidebar row or terminal header to an edge to split; drop in the center to swap or replace. Splits nest, dividers resize with pointer or arrow keys, and the layout is saved in each browser. Phones display the focused terminal.
- Fit and exact columns/rows resize the real Session Host PTY; this also affects its other viewers. Auto restores the dimensions before the browser's first manual resize. Other attached writers can resize normally; there is no tmux-style window-size lock.
- Kill immediately force-stops the selected shell/process group. Closing a tile only removes it from the browser view.
- `?termId=<session UUID>` preserves the selected terminal in the URL. Light/dark preference is stored in the browser, with the shared Slate palette.

Hosts from older builds that omit optional viewport metadata use the snapshot dimensions, or an 80×24 fallback when there is no snapshot. New Session Hosts report their actual PTY size. Journal compaction can limit retained scrollback, just as it does for other Multiplex viewers.

## Development

The Vue/Nuxt/xterm view was adapted from the provided tmux-viewer project. Its layout and interactions are retained; the backend uses authenticated Multiplex Host connections. No terminal data is sent to a third-party service.

```sh
scripts/build/browser-terminals.sh
cargo run -p multiplex --bin multiplex
```

Bun is used **only** to build/check the frontend. The script type-checks, runs layout tests, generates the static app, and copies the bundle into the CLI crate. Rust's build script embeds those files into the application. Commit source and embedded assets together. Cargo/release builds use the checked-in bundle and do not need JavaScript tooling.

The browser reads through a read-only Host attachment. Input/resize request the writer lease only for the mutation and release it afterward. Destructive operations verify the expected Host instance ID. Names and IDs are data; browser requests never run shell commands or supply filesystem paths.
