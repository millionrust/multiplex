# Mobile Terminal Manual Verification

Use this checklist before a client demo or release candidate. The goal is to prove that desktop, iOS, and Android all connect directly to the same SSH target and attach to the same persistent tmux session.

## Prerequisites

- A reachable SSH host from the desktop and mobile device or simulator.
- `tmux` installed on the SSH host.
- A saved TermiRust desktop host with persistent tmux enabled.
- A known-host pin for that SSH endpoint in desktop TermiRust.
- A mobile vault exported from desktop TermiRust.
- The same mobile vault imported on iOS and Android.
- The host credential saved in iOS Keychain or Android Keystore-backed storage.

## Prepare The SSH Host

1. Connect to the target host with any SSH client.
2. Run:

```bash
command -v tmux
tmux -V
```

Pass: both commands print tmux information.

Fail: install tmux on the target host, then retry.

## Prepare Desktop TermiRust

1. Open the saved host in TermiRust.
2. Enable persistent session for the host.
3. Set a deterministic session name, for example:

```text
mobile-demo
```

4. Connect from desktop TermiRust.
5. In the terminal, run:

```bash
tmux display-message -p '#S'
```

Pass: output is `mobile-demo`.

6. Create a visible marker:

```bash
echo desktop-sees-this > ~/termirust-mobile-marker
cat ~/termirust-mobile-marker
```

Pass: output is `desktop-sees-this`.

## Export Mobile Vault

1. In desktop TermiRust, open Settings or the export area.
2. Choose `Export Mobile Vault`.
3. Enter a passphrase and confirm it.
4. Save the file as:

```text
termirust-mobile-vault.encrypted.json
```

Pass: desktop reports exported host, identity, vault, and known-host counts.

Fail: if known-host count is `0`, connect to the host once from desktop so TermiRust pins the host key, then export again.

## Verify iOS

1. Open the iOS app.
2. Enter the mobile vault passphrase.
3. Import `termirust-mobile-vault.encrypted.json`.
4. Select the host.
5. Confirm the host screen shows `Host key pinned`.
6. Save the SSH credential:
   - For password auth, enter the SSH password and tap `Save Credential`.
   - For key auth, tap `Import Key File` or paste the private key, then save.
7. Tap `Connect`.
8. Run:

```bash
tmux display-message -p '#S'
cat ~/termirust-mobile-marker
echo ios-sees-this >> ~/termirust-mobile-marker
cat ~/termirust-mobile-marker
```

Pass:

- Session name is `mobile-demo`.
- Marker contains `desktop-sees-this`.
- New marker line contains `ios-sees-this`.

Fail:

- `Host key not pinned`: re-export the mobile vault after desktop has trusted the host.
- `Host key mismatch`: do not bypass. Confirm the target host identity and remove/update the desktop known-host pin only if the host key change is legitimate.
- Missing credential: save the credential again on iOS.

## Verify Android

1. Open the Android app.
2. Enter the mobile vault passphrase.
3. Import `termirust-mobile-vault.encrypted.json`.
4. Select the host.
5. Confirm the host screen shows `Host key pinned`.
6. Save the SSH credential:
   - For password auth, enter the SSH password and tap `Save Credential`.
   - For key auth, tap `Import Key File` or paste the private key, then save.
7. Tap `Connect`.
8. Run:

```bash
tmux display-message -p '#S'
cat ~/termirust-mobile-marker
echo android-sees-this >> ~/termirust-mobile-marker
cat ~/termirust-mobile-marker
```

Pass:

- Session name is `mobile-demo`.
- Marker contains `desktop-sees-this` and `ios-sees-this` if iOS was tested first.
- New marker line contains `android-sees-this`.

Fail:

- `Host key not pinned`: re-export the mobile vault after desktop has trusted the host.
- Host-key failure or mismatch: stop and verify the target host key.
- Missing credential: save the credential again on Android.

## Persistence Checks

Run these checks from iOS and Android.

1. Start a long-running command:

```bash
while true; do date; sleep 5; done
```

2. Close the mobile app or disconnect.
3. Reopen the app and reconnect.

Pass: reconnect attaches to the same tmux session. The command is still running or its output is still visible in tmux history.

4. Stop the command with `Ctrl-C`.

## Host-Key Mismatch Check

Only run this against a disposable test host.

1. Export a mobile vault with a known-host pin.
2. Replace or regenerate the SSH host key on the test host.
3. Try to connect from iOS and Android.

Pass: both apps block connection and show a host-key warning.

Fail: any app that proceeds to authenticate has failed the security bar.

## Credential Removal Check

1. Remove the saved mobile credential in the app.
2. Try to connect again.

Pass: connection does not authenticate and the app reports a missing credential.

Fail: the app reconnects without asking for the credential.

## Result Template

Record the result after every manual run:

```text
Date:
Tester:
Desktop commit:
iOS commit:
Android commit:
Target OS:
tmux version:
Desktop persistent session: pass/fail
iOS vault import: pass/fail
iOS SSH tmux attach: pass/fail
iOS host-key mismatch block: pass/fail/not run
Android vault import: pass/fail
Android SSH tmux attach: pass/fail
Android host-key mismatch block: pass/fail/not run
Credential removal block: pass/fail
Notes:
```
