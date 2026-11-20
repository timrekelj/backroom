# thehideout
A Discord-style chat app for me and my friends, built to learn handling lots of data

## Development

Start ScyllaDB (single dev node) and a local S3-compatible object store
(SeaweedFS) for avatars and guild icons. A one-shot `seaweedfs-init` container
creates the `thehideout-media` bucket; both services listen on 127.0.0.1 only:

```sh
docker compose up -d
```

Run the server (applies schema migrations on startup, listens on `0.0.0.0:8080`):

```sh
cd server && cargo run
```

The dev Scylla runs with 2 cores and 2 GB (`--smp 2 --memory 2G` in
`docker-compose.yml`); the integration tests are DDL- and write-heavy and slow
down a lot on one core. After changing these flags run `docker compose up -d scylla`
(the data volume is kept). The tests share one migrated keyspace
(`test_shared_<hash>`), so run only one `cargo test` against the same Scylla at a time.

Run the tests (unit tests plus integration tests against the Scylla and S3
containers, so `docker compose up -d` must be running; the media tests fail
with a clear message if the object store is unreachable):

```sh
cd server && cargo test
```

Requires `buf` on `PATH` for protobuf codegen.

## Configuration

Settings are environment variables. Copy `.env.example` to `.env` (gitignored)
and edit it; the server loads the repo-root `.env`, and real environment
variables override it. `.env.example` documents every variable and its default.
Never commit `.env`.

## Media storage (S3)

Avatars and guild icons are processed (centre-cropped, 256×256, lossless WebP) and
stored in an S3-compatible bucket; clients load them from `GET /media/{key}` on the
server (public, immutable cache headers). If the object store is unreachable at
startup the server logs one warning and keeps running: the media RPCs answer
`UNAVAILABLE` and `/media` answers 503 until it is back.

| Variable | Default (local compose) | Meaning |
| --- | --- | --- |
| `THEHIDEOUT_S3_ENDPOINT` | `http://127.0.0.1:8333` | S3 API endpoint |
| `THEHIDEOUT_S3_REGION` | `us-east-1` | signing region |
| `THEHIDEOUT_S3_BUCKET` | `thehideout-media` | bucket (must exist) |
| `THEHIDEOUT_S3_ACCESS_KEY` | `thehideout-dev` | access key id |
| `THEHIDEOUT_S3_SECRET_KEY` | `thehideout-dev-secret` | secret key |
| `THEHIDEOUT_S3_PATH_STYLE` | `true` | `endpoint/bucket/key` addressing; set `false` for virtual-hosted style |
| `THEHIDEOUT_S3_PUBLIC_BASE` | unset | reserved for a CDN in front of `/media`; currently only logged |

The compose credentials are well-known development values, valid only for the
127.0.0.1-bound local service. Never reuse them anywhere reachable.

### Hosting on a real S3 provider

Any S3-compatible provider works (AWS S3, Cloudflare R2, Backblaze B2, Wasabi, a
self-hosted SeaweedFS/Garage, ...): create a private bucket, create an access key
limited to it, and set `THEHIDEOUT_S3_ENDPOINT`, `_REGION`, `_BUCKET`, `_ACCESS_KEY`
and `_SECRET_KEY`. For AWS S3 set `THEHIDEOUT_S3_PATH_STYLE=false` (and the bucket's
real region); R2 and most others accept either style with `us-east-1`/`auto`
regions as documented by the provider. The bucket stays private: the server
streams objects through `/media`, so no public bucket policy is needed.
