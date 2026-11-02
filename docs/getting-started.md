# Getting Started

Install Rust 1.87 or later with [rustup](https://rustup.rs/). A Docker engine
with Docker Compose is optional. A running tcache service is needed only for
requests that omit both matrix fields.

## Build

Build and test the default configuration:

```sh
cargo build
cargo test
```

## Run the server

`TCACHE_BASE_URL` is optional. The example below supplies its own matrix, so no
tcache configuration or running tcache service is needed:

```sh
cargo run
```

The server listens on `0.0.0.0:8080` by default.

## Submit a Job

In another terminal, check the server and submit the example request. Use a new
`job_id` in [`examples/basic-request.json`](examples/basic-request.json) when
repeating it.

```sh
curl http://127.0.0.1:8080/health

curl -i -X POST http://127.0.0.1:8080/integration/jobs \
  -H 'Content-Type: application/json' \
  --data-binary @docs/examples/basic-request.json
```

The submission returns `202 Accepted`.

## Read the result

Poll the Job until its status is `completed`, `failed`, or `cancelled`:

```sh
curl http://127.0.0.1:8080/integration/jobs/readme-matrix-example
```

The example is self-contained. Its supplied matrix bypasses tcache and should
complete with the order `hotel -> park -> museum -> station` and 35 minutes of
travel.

See the [HTTP API reference](api.md) for synchronous compatibility, event
streams, cancellation, errors, and the complete request and response fields.
Runtime and optional solver setup are documented in the
[configuration reference](configuration.md). Local workflows and operations
are covered by the [development guide](development.md) and
[deployment guide](deployment.md).
