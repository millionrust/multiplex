# N15 Qualification Evidence

Date: 2026-09-03

## Automated Result

The following gates passed on macOS arm64:

- metadata restore and Session Host lease crash matrices;
- derived-index five-point crash recovery;
- relay atomic metadata crash recovery;
- update metadata tamper, expiry, freeze, replay, rollback, root rotation, and atomic-state tests;
- bounded Host protocol fuzz smoke;
- 1,000 detach/reattach cycles and exact 32-Host cap;
- 1 MiB and 64 MiB journal throughput (64 MiB: 1,653.9 MiB/s);
- 100 MiB accessibility parser throughput (156.79 MiB/s, bounded to 2,000 lines);
- relay loopback budgets at 1, 10, and 100 pairs;
- desktop terminal startup/input/frame/output budgets;
- isolated-browser hostile-page, cancellation, and profile-cleanup tests.

Run the same set with:

```bash
./scripts/verify-launch-qualification.sh --automated
```

The consolidated command passed after the N14 release-workflow and native-notification changes.
The endurance runner's minimum-duration guard was also exercised: `--hours 1` exited with status 2
and did not start a partial soak.

## Required Long-Running Evidence

The bounded endurance runner refuses durations shorter than 48 hours:

```bash
./scripts/soak-session-relay.sh --hours 48
```

Sustained libFuzzer runs, 48-hour Session/relay endurance, Android and iOS physical-device
lifecycle/accessibility journeys, Linux/Windows package journeys, external SSH interoperability,
and signed upgrade/rollback drills remain release gates. Short local executions are not recorded as
substitutes for those results.
