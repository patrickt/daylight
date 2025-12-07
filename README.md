# daylight

Daylight is a blazing-fast syntax highlighting RPC server built on top of [axum](https://github.com/tokio-rs/axum), [flatbuffers](https://flatbuffers.dev), and [tree-sitter-highlight](https://tree-sitter.github.io/tree-sitter/3-syntax-highlighting.html).

It is highly experimental, but will someday be suitable for use at scale. Probably don't use this, unless you want to help build it.

## Design goals

* _Zero-copy._ Absolutely no copies of source code should be made once a request is being handled. Flatbuffers allows us to avoid a deserialization step, so the byte buffers specified as part of the payload can be operated on directly by tree-sitter-highlight. (This eliminates [Twirp](https://github.com/github/twirp-rs) and [tonic](https://github.com/hyperium/tonic) as implementation platforms, as they require serialization/deserialization.)
* _Byte buffers, not strings._ Verifying that a large file is valid UTF-8 can take too long (on the client or the server) for large documents, so source is delivered as bytes. If you pass invalid UTF-8, you should get a good error message, but it should come from tree-sitter internals, not an explicit check.
* _Failure-tolerant._ One pathologically large file in a batch should not prevent the rest of the batch from highlighting. (This is not really implemented yet.)
* _Highly concurrent._ It uses Axum's `spawn_blocking` function to run many highlighting tasks at once. The number of highlighting tasks can be configured with the `max_blocking_threads` property on the builder.
* _Client-friendly._ Any language with a Flatbuffers binding and an HTTP library should be able to write to this. Unfortunately, until the Rust flatbuffers crate supports RPC definitions, this project cannot define the RPC interface in the schema. Right now there is only one interesting endpoint so that is fine. (This eliminates [tarpc](https://github.com/google/tarpc) as an option, because it supports only Rust clients.)
* _Wide language support._ All the official tree-sitter languages should work, and any reasonably-up-to-date community language should work, too.

## Running

In one tab: `cargo run --bin daylight-server <ADDRESS>`
In another: `cargo run --bin daylight-client [-l LANGUAGE] <ADDRESS> PATH`

The client will, for now, call out to `/v1/html` and write a file to /tmp containing the HTML. I haven't actually written any of the CSS required to display highlights in color, but you can check the output and see that classes are set.

You can look in the flatbuffer specification file in `daylight.fbs` to see the types of returns and requests.

## Environment variables

- `DAYLIGHT_WORKER_THREADS`: how many highlighting workers may be allowed. If all workers are busy, highlighting requests will be queued. Default: 512.
- `DAYLIGHT_DEFAULT_PER_FILE_TIMEOUT_MS`: how long an individual file is allowed to take before it (and other pending requests) are cancelled, if not specified in a request.
- `DAYLIGHT_MAX_PER_FILE_TIMEOUT_MS`: the maximum timeout value; requests with a larger value will return 400 Bad Request.

These can also be passed as command line arguments.

Daylight also supports OpenTelemetry tracing through the use of the [OpenTelemetry environment variable specification.](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/). If you don't want such tracing, provide `OTEL_SDK_DISABLED=true`.

## Limitations

The maximum request size is 2GB. This is a limitation of Flatbuffers, but should be enough for real world use.
The maximum size of any one file is 256MB. There is no limitation on number of files in a request, aside from those inherent in Flatbuffers.
tree-sitter supports UTF-16, but tree-sitter-highlight doesn't. A pity, that. UTF-8 is required.

## Future work

* Highlighting to HTML is easy. A more interesting view of syntax highlighting is to return structured data for use in rich environments such as text editors. What that looks like is yet to be determined.
* JSON would be friendly, if we can avoid slowness.
* Websockets, or some such streaming mechanism, would be cool.
* Themes?

## License

MIT, like tree-sitter. Buy me some luxury goods if somehow you use this to get rich.
