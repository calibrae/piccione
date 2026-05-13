# Memory accounting — how to read signalui's real footprint

## TL;DR
- `ps -axo rss` lies on macOS by ~2-3× for Cocoa apps. It counts shared
  read-only framework code (`__TEXT`, `__OBJC_RO`, `__LINKEDIT`) as
  resident in every process that maps them, even though those pages
  are physically loaded into RAM exactly once.
- The honest number is the **physical footprint** from `vmmap` or
  Activity Monitor's "Memory" column.
- For signalui (release, post-pair, full contact picker rendered):
  - `ps` says 244 MB. **Actual is ~100 MB.**

## The right command
```bash
PID=$(pgrep -fx 'src-tauri/target/release/signalui')
vmmap --summary "$PID" | grep "Physical footprint"
# or
footprint "$PID" | head -3
```

## Honest breakdown (steady-state release build, after this session's opts)

| Process | `ps` RSS | Physical footprint |
|---|---|---|
| `target/release/signalui` (Rust core) | 123 MB | **42 MB** |
| WebKit.GPU | 30 MB | 11 MB |
| WebKit.Networking | 24 MB | ~6 MB |
| WebKit.WebContent | 67 MB | 41 MB |
| **Total** | **244 MB** | **~100 MB** |

The 144 MB gap is **shared OS framework code**. Most macOS apps that link
against AppKit + WebKit see this. `vmmap` confirms it: `__TEXT = 466 MB
resident, ReadOnly portion of Libraries = 579 MB`. The vast majority of
that is system frameworks (`__OBJC_RO` alone = 50 MB) shared with every
other Cocoa process on the machine.

## Optimizations that actually helped

| Surgery | Where | Impact |
|---|---|---|
| `mimalloc` as global allocator | signalui Cargo.toml + lib.rs | Storage-sync transient spike 311 MB → 128 MB. Steady-state ~unchanged. |
| `SqlitePoolOptions::max_connections(3)` | calibrae/presage `presage-store-sqlite/src/lib.rs` | Cap connection pool ceiling. SQLite cache budget worst-case 80 MB → 6 MB. |
| `PRAGMA cache_size = -2000` | same | Per-connection page cache 8 MB → 2 MB. |
| `[profile.release] lto = "fat", codegen-units = 1, strip = "symbols"` | signalui Cargo.toml | Binary 25.5 MB → 16 MB on disk. Marginal RSS effect. |

## Optimizations that were proposed but did NOT pay off

- **`Arc<SqliteStore>` instead of three `Manager` clones**: SqliteStore's
  `db: SqlitePool` is already Arc-internally-shared by sqlx, so the
  three clones already share the same connection pool. No saving.
- **Lazy-loading the conversations list**: at ~100 bytes per entry × 168
  = ~16 KB. Not worth the complexity.

## What's left

The remaining steady-state footprint (~100 MB) is essentially WebKit
WebContent (41 MB) + Rust presage state (42 MB) + WebKit helpers (~17
MB). To push significantly lower we'd need:

1. **Drop WebKit entirely** — port the UI to a Rust-native renderer
   (`iced` is the safest bet; `floem` is the prettiest). Estimate: 2-3
   weekend rewrite of `src/`, no backend changes. Lands around 60-80 MB
   total physical footprint.
2. **Trim presage state** — share one `Manager` instead of three clones
   (the `state: Arc<Registered>` part is already shared, but each clone
   keeps its own `OnceLock<PushService>` + websocket Arc<Mutex<>> entries).
   Marginal — single-digit MB.

Both are heavy surgeries for cosmetic gains. The current 100 MB is
already better than the original CLAUDE.md `<100 MB idle` goal.

## How to verify after future changes

```bash
# Boot, complete one full cycle (pair OR load existing registration,
# storage-sync, open a conversation), then:
PID=$(pgrep -fx 'src-tauri/target/release/signalui')
vmmap --summary "$PID" | grep -E "Physical footprint"

# For the WebKit children, find them by start time:
ps -axo pid,lstart,comm | grep WebKit | awk '/'$(date +%Y)'/ {print $1}' | while read p; do
  echo "pid=$p $(footprint $p 2>/dev/null | grep -m1 Footprint:)"
done
```

`top -l 1 -pid $PID -stats pid,rsize,mem,purg,vsize,command` also shows
the right number under the `MEM` column (it reports physical footprint,
not RSS). Same as Activity Monitor.
