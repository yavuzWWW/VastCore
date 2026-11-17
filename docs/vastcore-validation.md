# VastCore development acceptance

The fork retains upstream Minecraft behavior and licensing. The production-dev branch is an unqualified development branch, not a production release.

## Every development push

- Windows x86-64 and Linux x86-64: workspace Rust tests, focused doctests, strict Clippy on changed storage/protocol/test packages, formatting.
- Both platforms: build the standalone server plus upstream headless integration runner.
- Independent Python TCP implementation checks Java status and ping on the real executable, normal console stop, readable gzip level metadata and restart with the same scratch world.
- Upstream tester exercises Java packet login, movement, commands, loot tables, world interaction, shearing, mobs, player actions, disconnect cleanup, sign authorization, embedded shutdown and required GameTests.
- Saved-state assertions check the actual NBT hotbar selection after embedded shutdown, not just existence of a file.
- All scratch server configurations disable telemetry and external update checks. No live worlds or accounts are needed.
- A failed or skipped matrix is not a pass. Logs and the exact commit identify the test evidence.

## Release qualification (not implemented as an automatic release)

| Area | Required evidence |
|---|---|
| Java protocol | Current supported protocol, compression, encryption, online authentication, reconnect, transfers and an official client play-through |
| Bedrock | Supported transport, current client authentication, join, movement, inventory, cross-play and disconnect |
| Gameplay parity | Reference vanilla outcomes for crafting, inventory, redstone, combat, mobs, dimensions, commands and datapacks |
| Data safety | Crash and disk-full tests, corrupted chunk recovery, stale-save ordering, world migration and backup restoration |
| Hosting | Linux and Windows release binary, service start/stop, clean install, upgrade, restore and rollback |
| Plugin compatibility | Native and Wasm ABI tests, event correctness, bounded shutdown; no Bukkit/Fabric compatibility claim |
| Performance | Fixed workload and hardware, upstream and fork comparison, peak RSS, tick p95/p99, throughput and a sustained 72-hour soak |
| Client visuals | Real-client inventory/rendering, particles, sounds, GUI and prediction checks with captured evidence |

Passing headless GameTests does not establish full vanilla parity or replace a real client. The standalone smoke test validates startup and status, not online authentication or gameplay. The gzip check is not a full NBT semantic verification. No public release should be advertised without the release evidence above.
