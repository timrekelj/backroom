# thehideout
A Discord-style chat app for me and my friends, built to learn handling lots of data

## Development

Start ScyllaDB (single dev node):

```sh
docker compose up -d
```

Run the server (applies schema migrations on startup, listens on `0.0.0.0:8080`):

```sh
cd server && cargo run
```

Run the tests (unit tests plus integration tests against the Scylla container):

```sh
cd server && cargo test
```

Requires `buf` on `PATH` for protobuf codegen. Configuration is via env vars:
`THEHIDEOUT_LISTEN_ADDR`, `THEHIDEOUT_SCYLLA_NODES`, `THEHIDEOUT_KEYSPACE`, `THEHIDEOUT_WORKER_ID`, and `RUST_LOG`.
