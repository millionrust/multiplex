# Remote terminals: reaching your desktop terminals from the mobile app

Status: **partly built.** On macOS and Linux, tmux sessions are listed to paired devices
and can be watched and typed into, and the desktop app can set up new terminals to start
inside tmux after showing you the exact file changes, over every Controller route. On macOS
the local network listener can keep running after you quit the app. Windows is not built
yet; see "What has to be built".

This is the guide a person follows when they want the terminals on their computer to
show up on their phone.

## The rule that shapes everything

A program can only read or write a terminal whose pseudo-terminal it owns. A terminal
that Terminal.app, Zed, iTerm2, or Windows Terminal started belongs to that app, and
nothing can adopt it afterwards. Linux has `reptyr`, which reparents a process with
`ptrace`; it is not macOS-compatible, and System Integrity Protection blocks that class
of trick anyway. Windows has no equivalent for an existing ConPTY either.

So the question is never "how do we capture the terminals that are already open". It is
"what do new terminals run inside, so that a second client can attach to them". Two
answers work:

- **A multiplexer.** A shell started inside `tmux` belongs to the tmux server, which is
  built to serve several clients. TermiRust attaches as one more client.
- **The session host.** `termirust-session-host` owns a PTY in its own process, keeps a
  replayable journal, and outlives the app. This is what TermiRust's own durable
  sessions already use.

Terminals open *before* you turn this on stay unreachable. Open them again and they are
reachable from then on.

## What works today

- Pairing a phone by typing a six-digit code the desktop shows (CPace bound into Noise XX;
  see `docs/decisions/controller-security-v1.md`), or by scanning an offer and comparing a
  `XXXX-XXXX` code. Host private key lives in the OS credential store.
- Three routes: private LAN/VPN, Controller-over-SSH, and a self-hosted relay.
- `ListSessions`, `Attach` with replay from a watermark, `Input`, and `Resize`, gated by
  capability bits and a single-writer lease. A device without `SendInput` is read-only
  in the protocol, not just in the UI.
- Live desktop panes (the terminals in the desktop window) are published to the
  Controller and are attachable on every route. The SSH and relay routes find them through
  a user-only pointer file the running app publishes.
- **tmux sessions**, when "Show tmux sessions" is on: every session on your default tmux
  server appears in the phone's session list as a live terminal, including sessions
  TermiRust did not create, on every route. Watching and typing work; a
  tmux session is never resized or ended by the phone. Requires tmux 3.2 or later.
- **Setup for new terminals**, which starts new tabs in Terminal, Zed, iTerm2, Ghostty,
  WezTerm, and the VS Code terminal inside tmux. Previewed, applied, and removed from the
  desktop app.
- Local panes marked persistent already run inside `tmux new-session -A -s <name>`.

Not yet: approval prompts answered from the phone (`Approval` returns an error on both
backends), and any relay UI on the desktop (relay is CLI-only).

Remote exposure is gated in the decision records (D06 and an independent cryptographic
review). Treat this guide as LAN first.

## Opting in

Nothing changes on install. Every step below is in **Devices** (or Settings → Remote
Devices), and nothing touches your files until you have seen the change.

1. **Turn on remote access and pair a phone.** Under "Remote access", choose **Turn on**.
   The listener accepts connections on every private address the computer has (Wi-Fi,
   Ethernet, and VPNs such as Tailscale), on one port, and follows addresses as networks
   change. Choose **Pair phone**: the desktop shows a six-digit code. On the phone, pick
   the computer from the list of computers on the same network, or, over Tailscale, type
   the address the desktop shows (for example `mac.tail1234.ts.net:55123`), then type the
   code. A code allows three attempts and expires after five minutes. Scanning a QR code
   and comparing an eight-character code remains available under **Other ways to pair**.
   New devices are observe-only; granting input is a separate, explicit toggle per
   device.
2. **Show tmux sessions.** Under "Terminals opened in other apps", choose **Show**. The
   listener restarts, so a connected phone reconnects once.
3. **Open new terminals in tmux.** Choose **Review changes**. The app lists every file it
   will create or edit, with the exact lines, and writes them only when you choose
   **Apply changes**. If a file changes between your review and Apply, nothing is written
   and you are asked to review again.
4. **Check setup.** The app starts a throwaway tmux session, confirms it appears in the
   listing a phone would see, and ends it. Your own sessions are left alone.

Open a new Terminal or Zed tab and it appears on the phone.

## What the setup writes

### macOS and Linux

Requires tmux 3.2 or later. The app looks for it at `$TERMIRUST_TMUX_PATH`, on `PATH`,
and in the usual Homebrew and system locations, and writes the absolute path it found
into the init file (such as `/opt/homebrew/bin/tmux`, not the versioned Cellar directory
behind it, so an upgrade keeps working) so tabs started with a minimal `PATH` still find it. If tmux is missing, the
section tells you how to install it and leaves the setup unavailable.

The tmux status bar is turned off for these sessions only; your other tmux sessions keep theirs.

Scrolling stays native. tmux normally switches the terminal to its alternate screen, so
the terminal app has no scrollback and every wheel event goes through tmux's copy mode.
The init file sets `terminal-overrides[97]` to `*:smcup@:rmcup@` on the tmux server
before the session starts, so output lands in the terminal app's own scrollback and the
trackpad scrolls it directly. This is a server option: it also applies to other clients
of the same tmux server, and removing the setup unsets it. Tabs attached before you apply
the setup keep the old behavior until they reattach.

One app-owned init file per shell, safe to delete —
`~/.config/termirust/shell-init.zsh` for zsh (and `shell-init.bash` for bash):

```zsh
# Managed by TermiRust. Turn off "Open new terminals in tmux" in TermiRust, or delete this file and the marked block in your shell startup file.
if [[ -o interactive && -z "$TMUX" && -z "$TERMIRUST_NO_WRAP" ]]; then
  case "$TERM_PROGRAM" in
    Apple_Terminal|zed|iTerm.app|ghostty|WezTerm|vscode)
      if [[ -x '/opt/homebrew/bin/tmux' ]]; then
        '/opt/homebrew/bin/tmux' start-server \; set-option -s 'terminal-overrides[97]' '*:smcup@:rmcup@' \; new-session -s "termirust-${PWD:t}-$$" \; set-option status off && exit
      fi
      ;;
  esac
fi
```

And one marked block at the end of `~/.zshrc` (or `~/.bashrc`), which the app finds and
removes by its markers:

```zsh
# >>> termirust remote terminals >>>
[ -f "$HOME/.config/termirust/shell-init.zsh" ] && . "$HOME/.config/termirust/shell-init.zsh"
# <<< termirust remote terminals <<<
```

The app sets up your login shell, plus bash or zsh if its startup file already exists. A
startup file that a dotfile manager symlinks is edited through the link, and its
permissions are kept.

The guards matter:

- `-o interactive` (bash: `$- == *i*`) skips scripts and CI.
- `-z $TMUX` prevents nesting when you run tmux yourself.
- `TERM_PROGRAM` limits it to terminal apps; anything else is left alone.
- `&& exit` instead of `exec`: if tmux cannot start, you keep a plain shell rather than a
  tab that closes the moment it opens. Detaching (`Ctrl-b d`) closes the tab and leaves
  the session running for the phone.
- `TERMIRUST_NO_WRAP=1` is the escape hatch for any tool that misbehaves inside a
  multiplexer — set it in that tool's environment, not globally.

No `~/.tmux.conf` change is needed. The phone attaches with
`tmux attach-session -f ignore-size`, so it never takes part in window sizing.

### Windows

Not built yet. There is no tmux, and PowerShell cannot replace itself with another process
the way a POSIX shell can, so the session host has to own the terminal from the start.

The intended setup adds a Windows Terminal profile whose command line is a TermiRust shim,
and optionally makes it the default profile:

```json
{
  "name": "PowerShell (TermiRust)",
  "commandline": "termirust.exe shell -- pwsh.exe -NoLogo",
  "startingDirectory": "%USERPROFILE%",
  "icon": "ms-appx:///ProfileIcons/pwsh.png"
}
```

`termirust shell` would create a session-host-owned ConPTY, attach your console to it, and
leave the session running when the window closes. For WSL shells, the macOS/Linux tmux
setup above applies inside the distribution.

### Keeping sessions reachable when the app is closed

tmux sessions survive the app, but something has to accept Controller connections:

- **Controller-over-SSH** needs nothing extra: `sshd` starts the bridge for each connection,
  so tmux sessions are reachable whenever the computer is on.
- **Self-hosted relay** needs `termirust relay-host run` running.
- **Local network (macOS):** under "Keep reachable when TermiRust is closed", choose
  **Run in background**. This installs a per-user LaunchAgent,
  `~/Library/LaunchAgents/com.termirust.desktop.controller-service.plist`, which runs
  `termirust controller-service run` at login. It serves already-paired devices on every private
  address; pairing a new device still needs the app. When you open TermiRust, the service
  hands the route to the app, and takes it back when the app quits. The same commands work
  from a terminal: `termirust controller-service install|remove|status`.
- **Windows:** not built yet; the intended setup is a per-user scheduled task at logon.

## Turning it off

Every change is reversible from the same section, or by hand:

1. **Review removal**, then **Remove from my files**. This deletes the marked block and
   the init files and leaves the rest of your startup file exactly as it was. By hand:
   delete the marked block in `~/.zshrc` and the files in `~/.config/termirust/`.
2. Choose **Hide** under "Show tmux sessions".
3. Choose **Stop running in background**, or run `termirust controller-service remove`.
4. Revoke paired devices. Revocation increments the epoch and closes live channels.

Existing tmux sessions keep running; `tmux kill-server` ends them.

## What this does not give you

- **Terminals opened before you turned it on.** Open them again.
- **Other apps' terminals, without the setup.** iTerm2's Python API and Terminal.app's
  AppleScript can mirror a tab and send text, but they need Automation and Accessibility
  permission, and they are per-app adapters rather than a general path. Not implemented.
- **The phone's own window size.** The phone sees the desktop's window geometry, clipped
  to its screen. When no desktop client is attached, tmux sizes the window to the phone.
- **Sessions on a non-default tmux server.** Only the default socket (honoring
  `TMUX_TMPDIR`) is listed; servers started with `-L` or `-S` are not.
- **Discovery over a VPN.** The listener announces itself with Bonjour (`_termirust._tcp`,
  named by an opaque identifier, not the computer name) only on Wi-Fi and Ethernet.
  Multicast does not cross Tailscale, so type the address once there; the phone then keeps
  every address it learns. The listener never opens a firewall hole and never binds a
  public, loopback, or wildcard address; macOS may prompt for the incoming-connection
  permission.

## What has to be built

Done: tmux session discovery and attach, the setup flow for macOS and Linux, route parity
(see `docs/decisions/controller-session-sources.md`), and the macOS background listener.

Remaining:

1. **Windows**: the `termirust shell` shim, the Windows Terminal profile writer, and a
   per-user logon task for the background listener.
