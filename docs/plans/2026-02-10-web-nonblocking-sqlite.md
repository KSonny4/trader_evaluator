# Web: Non-Blocking SQLite Reads

## Problem
The dashboard web server performs synchronous rusqlite reads inside async handlers.
Under high disk IO / slow reads, tokio worker threads get stuck (D-state) and the server stops responding (even to /login), causing Cloudflare Tunnel requests to hang.

## Goal
Ensure slow SQLite reads cannot block tokio worker threads.

## Plan
1. Add a failing concurrency test that simulates slow DB opens and asserts /login still responds while partials are in flight.
2. Refactor DB-backed handlers to run DB work in `tokio::task::spawn_blocking`.
3. Add a concurrency limiter (semaphore) for DB tasks to avoid unbounded blocking-thread growth.
4. Add a request-level timeout for DB tasks; on timeout return 503.
5. Run `cargo test -p web` to verify.
