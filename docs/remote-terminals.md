# Remote terminals: reaching your desktop terminals from the mobile app

Status: **built, awaiting testing on real Windows hardware.** On every platform, a terminal
opened with the **Multiplex profile** (Windows Terminal, Visual Studio Code, iTerm2) runs your
own shell in a session paired devices can list, watch, and type into, and the session outlives
its window. On macOS and Linux, tmux sessions are listed too, and new terminals can be started
inside tmux after showing you the exact file changes. On macOS and Windows the listener can keep
running after you quit the app.

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
  built to serve several clients. Multiplex attaches as one more client.
- **The session host.** `termirust-session-host` owns a PTY in its own process, keeps a
  replayable journal, and outlives the app. This is what Multiplex's own durable
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
  Multiplex did not create, on every route. Watching and typing work; a
  tmux session is never resized or ended by the phone. Requires tmux 3.2 or later.
- **The Multiplex terminal profile**: Devices adds a "Multiplex" profile to Windows Terminal,
  Visual Studio Code, and iTerm2. A terminal opened with it runs `multiplex-cli shell`, which
  starts your shell (PowerShell on Windows, your login shell elsewhere) in a Session Host, in the
  folder the window opened in, and attaches the window to it. Paired devices list it as
  "pwsh in projects". Nothing else changes: scripts, `cmd /c`, `powershell -Command`, and every
  other program keep starting the plain shell.
- **Setup for new terminals** (macOS and Linux, advanced), which starts every new tab in
  Terminal, Zed, iTerm2, Ghostty, WezTerm, and the VS Code terminal inside tmux. Previewed,
  applied, and removed from the desktop app.
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

Requires tmux 3.2 or later. The app looks for it at `$MULTIPLEX_TMUX_PATH`, on `PATH`,
and in the usual Homebrew and system locations, and writes the absolute path it found
into the init file (such as `/opt/homebrew/bin/tmux`, not the versioned Cellar directory
behind it, so an upgrade keeps working) so tabs started with a minimal `PATH` still find it. If tmux is missing, the
section tells you how to install it and leaves the setup unavailable.

Wrapped tabs should feel like the terminal they replaced, and your own tmux sessions should not
change. The setup never edits `~/.tmux.conf`. It writes an app-owned tmux configuration that each
wrapped tab sources right after `new-session`, so its options apply to that session:

- no status bar;
- the mouse on, so the wheel scrolls tmux's history (a tmux pane's history lives in tmux, not in
  the terminal app) and programs that ask for the mouse, such as Codex, receive it;
- a quiet grey selection and no `[n/n]` copy-mode position counter, also for new windows;
- dragging selects and copies to the clipboard (`pbcopy` on macOS), and the selection stays put
  while you scroll;
- a click clears the selection, and at the bottom of the history it also leaves copy mode, so
  typing reaches the program again. Scrolled back it stays in copy mode, because leaving it
  there would return the view to the live screen and the text would move under the click;
- scrolling back to the bottom leaves copy mode too, however it was entered;
- two lines per wheel step instead of five.

A wrapped tab also tells tmux what its terminal can do, because tmux only works that out for
itself from a terminal that answers its questions:

- `-u`, so tmux writes UTF-8 whatever the locale says;
- `-T RGB` when the terminal sets `COLORTERM` to `truecolor` or `24bit`, because tmux otherwise
  converts every 24-bit color to the nearest of 256 and the tab looks unlike the one it replaced.
  tmux 3.4 and newer usually work this out for themselves; tmux 3.2 and 3.3, which Ubuntu 22.04
  and Debian 12 ship, never do;
- `sync` for Zed, iTerm2, Ghostty, WezTerm, and the VS Code terminal, which understand
  synchronized updates. tmux then draws each frame between a begin and an end, so the terminal
  never paints half of one. Without it, a program that redraws constantly, such as a coding
  agent's spinner, flickers. Terminal.app has no such support and is left as it was.

The phone attaches with both features, since Multiplex's own terminals support them.

tmux key bindings belong to the whole server, so each binding checks the session name and keeps
tmux's default behavior in every other session. Applying the setup also updates sessions already
running; removing it deletes the file, unsets those options, and restores tmux's default
bindings. An earlier version set `terminal-overrides[97]` to keep tmux off the alternate screen,
which left programs such as Claude Code impossible to scroll; applying or removing the setup
clears it.

`~/.config/termirust/tmux.conf`, shared by every shell:

```tmux
# Managed by Multiplex for the tmux sessions it starts (named termirust-*). Turn off "Open new terminals in tmux" in Multiplex to remove it.
set-option status off
set-option mouse on
set-option -q -w copy-mode-position-format ''
set-option -q -w mode-style 'bg=#3b4252,fg=default'
set-hook after-new-window 'set-option -q -w copy-mode-position-format "" ; set-option -q -w mode-style "bg=#3b4252,fg=default"'
bind-key -T copy-mode MouseDragEnd1Pane 'if-shell -F "#{m:termirust-*,#{session_name}}" "send-keys -X copy-pipe-no-clear pbcopy ; send-keys -X stop-selection" "send-keys -X copy-pipe-and-cancel"'
# ...and the same guard for MouseDown1Pane, WheelUpPane, and WheelDownPane in copy-mode and copy-mode-vi
```

One app-owned init file per shell, safe to delete —
`~/.config/termirust/shell-init.zsh` for zsh (and `shell-init.bash` for bash):

```zsh
# Managed by Multiplex. Turn off "Open new terminals in tmux" in Multiplex, or delete this file and the marked block in your shell startup file.
if [[ -o interactive && -z "$TMUX" && -z "$MULTIPLEX_NO_WRAP" ]]; then
  case "$TERM_PROGRAM" in
    Apple_Terminal|zed|iTerm.app|ghostty|WezTerm|vscode)
      if [[ -x '/opt/homebrew/bin/tmux' ]]; then
        termirust_terminal=()
        [[ $COLORTERM == (truecolor|24bit) ]] && termirust_terminal+=RGB
        case "$TERM_PROGRAM" in
          zed|iTerm.app|ghostty|WezTerm|vscode) termirust_terminal+=sync ;;
        esac
        termirust_features=()
        (( ${#termirust_terminal} )) && termirust_features=(-T ${(j:,:)termirust_terminal})
        '/opt/homebrew/bin/tmux' -u $termirust_features new-session -s "termirust-${PWD:t}-$$" \; source-file -q '/Users/you/.config/termirust/tmux.conf' && exit
        unset termirust_terminal termirust_features
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
- `MULTIPLEX_NO_WRAP=1` is the escape hatch for any tool that misbehaves inside a
  multiplexer — set it in that tool's environment, not globally.

No `~/.tmux.conf` change is needed. The phone attaches with
`tmux attach-session -f ignore-size`, so it never takes part in window sizing.

### The Multiplex terminal profile (every platform)

This is the recommended way, and the only one on Windows. Under **Devices → Terminal profiles**,
each terminal app on this computer that can take a profile is listed with **Review adding**.
Reviewing shows the exact change; applying writes only that.

- **Windows Terminal** loads the profile from a file of its own,
  `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\Multiplex\multiplex.json`, so its
  `settings.json` is never edited. To open Multiplex whenever Windows Terminal opens, choose it
  under Settings → Startup → Default profile.
- **iTerm2** loads it the same way, from
  `~/Library/Application Support/iTerm2/DynamicProfiles/multiplex.json`.
- **Visual Studio Code** has no such folder, so one marked block is added at the top of your user
  `settings.json`, between `// >>> multiplex terminal profile >>>` and
  `// <<< multiplex terminal profile <<<`. When your settings already set
  `terminal.integrated.profiles.<platform>`, nothing is merged into them; the entry to add by
  hand is shown instead.

A window opened with the profile behaves like the shell itself: Ctrl-C, colours, full-screen
programs, and resizing work as before, and the window closes when the shell exits. Closing the
window instead leaves the session running for paired devices; `Ctrl-]` then `d` detaches
without closing it. The window holds the writer lease only while you type, so a paired device
can take over a terminal nobody at the computer is using; keys typed while a device is typing
ring the bell instead of mixing in.

`multiplex-cli shell` runs the program directly, without a session, when it has no terminal or
runs inside another Multiplex shell, so a profile never nests or wraps anything non-interactive.
It can also be run by hand: `multiplex-cli shell -- pwsh.exe -NoLogo`.

On Windows the sessions run in the Windows Session Host; see
`docs/decisions/windows-session-host.md`.

### Keeping sessions reachable when the app is closed

tmux sessions survive the app, but something has to accept Controller connections:

- **Controller-over-SSH** needs nothing extra: `sshd` starts the bridge for each connection,
  so tmux sessions are reachable whenever the computer is on.
- **Self-hosted relay** needs `termirust relay-host run` running.
- **Local network (macOS):** under "Keep reachable when Multiplex is closed", choose
  **Run in background**. This installs a per-user LaunchAgent,
  `~/Library/LaunchAgents/com.millionrust.multiplex.controller-service.plist`, which runs
  `termirust controller-service run` at login. It serves already-paired devices on every private
  address; pairing a new device still needs the app. When you open Multiplex, the service
  hands the route to the app, and takes it back when the app quits. The same commands work
  from a terminal: `termirust controller-service install|remove|status`.
- **Windows:** the same **Run in background** choice adds a value under the current user's
  `Run` key, which needs no administrator rights and shows under Settings → Apps → Startup,
  and starts the service at once. A notification-area icon says who is viewing or controlling
  the screen and offers **Open Multiplex** and **Stop until next sign-in**.

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
  every address it learns. It tries them together rather than in turn: the one that worked on
  this network first, then local addresses on the phone's own subnet, then Tailscale 250 ms
  later, so it stays on the local network at home even with Tailscale on
  (`docs/route-selection-plan.md`). The listener never opens a firewall hole and never binds a
  public, loopback, or wildcard address; macOS may prompt for the incoming-connection
  permission.

## What has to be built

Done: tmux session discovery and attach, the setup flow for macOS and Linux, route parity
(see `docs/decisions/controller-session-sources.md`), the background listener on macOS and
Windows, the Windows Session Host, `multiplex-cli shell`, and the terminal profiles.

Remaining:

1. **Windows, on real hardware**: a person checking the profile in Windows Terminal and VS Code,
   the background service and its tray icon, and a paired phone typing into a session.
2. **More terminal apps**: Ghostty and WezTerm take a command in their configuration files;
   Terminal.app profiles live in a property list.
3. **Moving a live session to a better route** and the Host sending its current addresses:
   phases 3–6 of `docs/route-selection-plan.md`.
