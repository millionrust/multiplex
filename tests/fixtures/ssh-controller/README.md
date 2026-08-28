# SSH Controller fixture

This fixture contains deliberately hostile OpenSSH configuration input used by
the strict argv and route verification. The production route always supplies
`-F none` and fixed forwarding, command, and multiplexing overrides, so none of
these directives may execute.

Run:

```sh
./scripts/test-controller-ssh.sh --fixture tests/fixtures/ssh-controller
```
