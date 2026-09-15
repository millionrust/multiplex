# Remote terminals: reaching your desktop terminals from the mobile app

Status: **proposed**. The Controller transport, pairing, attach, and writer leases
described in "What works today" are implemented. The opt-in setup flow, the shell
integration, and tmux session discovery are not; they are specified here and listed
under "What has to be built".

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
answers work, and the setup flow installs one of them:

- **A multiplexer.** A shell started inside `tmux` belongs to the tmux server, which is
  built to serve several clients. TermiRust attaches as one more client.
- **The session host.** `termirust-session-host` owns a PTY in its own process, keeps a
  replayable journal, and outlives the app. This is what TermiRust's own durable
  sessions already use.

Terminals open *before* you turn this on stay unreachable. Restart them once and they
are reachable from then on.

## What works today

- Pairing a phone with the desktop over Noise XX, with a `XXXX-XXXX` code compared by
  eye. Host private key lives in the OS credential store.
- Three routes: private LAN/VPN, Controller-over-SSH, and a self-hosted relay.
- `ListSessions`, `Attach` with replay from a watermark, `Input`, and `Resize`, gated by
  capability bits and a single-writer lease. A device without `SendInput` is read-only
  in the protocol, not just in the UI.
- Live desktop panes (the terminals in the desktop window) are published to the
  Controller and are attachable — **on the LAN route only**. Controller-over-SSH and the
  relay currently expose durable sessions only.
- Local panes marked persistent already run inside `tmux new-session -A -s <name>`.

Not yet: approval prompts answered from the phone (`Approval` returns an error on both
backends), any relay UI on the desktop (relay is CLI-only), and discovery of tmux
sessions the app did not create.

Remote exposure is gated in the decision records (D06 and an independent cryptographic
review). Treat this guide as LAN-and-SSH first.

## Opting in

The desktop app asks once, on first run, and never changes anything without showing the
change first. Nothing here happens silently at install time: an installer that edits
your shell startup file behind your back is how people lose an afternoon.

**Step 1 — Choose a level.**

| Level | What becomes reachable | What it changes |
| ----- | ---------------------- | --------------- |
| Off (default) | Nothing | Nothing |
| App terminals | Terminals opened inside TermiRust | Nothing outside the app |
| All new terminals | Every new terminal, including Terminal.app, Zed, and Windows Terminal | One guarded block in your shell startup file, or one terminal profile |

**Step 2 — Review the exact diff.** The app shows the file it will touch and the lines
it will add, then applies them only when you confirm.

**Step 3 — Pair a device.** Scan the QR code on the phone and compare the eight-character
code shown on both screens. New devices are observe-only; granting input is a separate,
explicit toggle per device.

**Step 4 — Verify.** The app opens a throwaway terminal, checks that it appears in the
session list, and reports what it found.

## What the setup writes

### macOS and Linux

Requires `tmux`. The app looks for it at `$TERMIRUST_TMUX_PATH`, on `PATH`, and in the
usual Homebrew and system locations; if it is missing, the setup offers to install it
rather than doing so on its own.

One file, owned by the app and safe to delete:

`~/.config/termirust/shell-init.zsh` (and a `.bash` sibling)

```zsh
# Managed by TermiRust. Delete this file and the block in ~/.zshrc to remove.
if [[ -o interactive && -z $TMUX && -z $TERMIRUST_NO_WRAP ]] && command -v tmux >/dev/null; then
  case $TERM_PROGRAM in
    Apple_Terminal|zed|vscode|iTerm.app|ghostty)
      exec tmux new-session -A -s "termirust-${PWD:t}-$$"
      ;;
  esac
fi
```

And one guarded block appended to `~/.zshrc`, written between markers so the app can find
and remove it later:

```zsh
# >>> termirust remote terminals >>>
[ -f "$HOME/.config/termirust/shell-init.zsh" ] && . "$HOME/.config/termirust/shell-init.zsh"
# <<< termirust remote terminals <<<
```

The guards matter. `-o interactive` skips scripts and CI. `-z $TMUX` prevents nesting
when you run tmux yourself. `TERMIRUST_NO_WRAP=1` is the escape hatch for any tool that
misbehaves inside a multiplexer — set it in that tool's environment, not globally.

Recommended in `~/.tmux.conf`, because the defaults fight a phone attaching to a desktop
session:

```tmux
set  -g mouse on
set  -g window-size latest
setw -g aggressive-resize on
```

### Windows

There is no tmux, and PowerShell cannot replace itself with another process the way a
POSIX shell can. The session host owns the terminal instead, from the start.

The setup adds a Windows Terminal profile whose command line is the TermiRust shim, and
optionally makes it the default profile:

```json
{
  "name": "PowerShell (TermiRust)",
  "commandline": "termirust.exe shell -- pwsh.exe -NoLogo",
  "startingDirectory": "%USERPROFILE%",
  "icon": "ms-appx:///ProfileIcons/pwsh.png"
}
```

`termirust shell` creates a session-host-owned ConPTY, attaches your console to it, and
leaves the session running when the window closes. Sessions started this way are visible
over every route, not only the LAN one, because they are durable sessions rather than
live app panes.

For WSL shells, the macOS/Linux tmux block above applies inside the distribution.

### Keeping sessions reachable when the app is closed

Durable sessions survive the app, but something has to accept Controller connections. The
setup offers to install a background service:

- macOS: a LaunchAgent in `~/Library/LaunchAgents`, running at login, user-level.
- Windows: a per-user scheduled task at logon.

Decline it and remote access works only while the desktop app is running.

## Turning it off

Every change is reversible from the same settings panel, or by hand:

1. Delete the marked block in `~/.zshrc` and the `~/.config/termirust/` file (or remove
   the Windows Terminal profile).
2. Remove the LaunchAgent or scheduled task.
3. Revoke paired devices. Revocation increments the epoch and closes live channels.

Existing tmux sessions keep running; `tmux kill-server` ends them.

## What this does not give you

- **Terminals opened before you turned it on.** Restart them.
- **Other apps' terminals, without the wrapper.** iTerm2's Python API and Terminal.app's
  AppleScript can mirror a tab and send text, but they need Automation and Accessibility
  permission, and they are per-app adapters rather than a general path. Not implemented.
- **Two clients at one size.** tmux sizes a window for its clients. Attach from a phone
  to a session you are using on a large display and the layout reflows for both. Attach
  read-only (`tmux attach -r`) to look without disturbing it, and take control
  deliberately.
- **Automatic discovery on the network.** The LAN listener never advertises itself, never
  opens a firewall hole, and binds only private interfaces on a high port. You supply the
  address when pairing, and macOS may prompt for the incoming-connection permission.

## What has to be built

In the order that delivers value soonest:

1. **tmux session discovery.** A session source that runs `tmux list-sessions` and
   exposes each session to `ListSessions`, attaching through a session host that runs
   `tmux attach -t <name>`. This is what turns a wrapped Terminal.app tab into a row on
   the phone. Everything else in this guide already exists in some form.
2. **The setup flow** described above: level picker, diff preview, idempotent writer with
   markers, verification, and revert.
3. **The `termirust shell` shim** and the Windows Terminal profile writer.
4. **Route parity**, so Controller-over-SSH and the relay see live panes, not only durable
   sessions.
5. **Background service installers** for LaunchAgent and the scheduled task.
