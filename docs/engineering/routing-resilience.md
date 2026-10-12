# Routing and resilience

## Network boundary

AI Router's local proxy listens only on loopback and protects the Codex projection with a local
gateway token. The text-inference entry point is the Responses API; the project does not claim
compatibility with the full OpenAI API. A user-saved base URL may be an API prefix or a complete
address ending at a protocol endpoint; internally only the prefix is stored, and exactly one final
endpoint is derived (`/responses` for Responses, `/chat/completions` for Chat Completions). `/v1` is
never guessed automatically.

Each route selects one upstream protocol. A Responses route forwards the raw request bytes to
`/responses`; a Chat Completions route translates the Responses request into a Chat request for that
single attempt only, and translates the upstream response back into Responses events. A translation
failure is a local client error: no upstream request is sent, route health is not penalized, and the
router does not silently move to the next route.

Once a request enters the proxy, it reads one immutable routing snapshot. An attempt binds the route,
base URL, API key, balance-script settings, and service-tier policy; configuration updates affect
only later snapshots and cannot mix old and new fields within an in-flight request.

## Automatic fallback

Automatic fallback uses an ordered boundary of participating routes. Only failures that are
explicitly classified as retryable and that have not yet committed a response to Codex may try the
next route. Once downstream headers/body have started to flow, the system cannot hide a partial
response and switch upstream. Image generation uses an independent single route and does not reuse
text automatic fallback.

A successful switch must persist the new active route; if persistence fails, the current request
fails, and the router cannot claim in memory alone that the switch happened. Concurrent requests
avoid letting an old request overwrite the user's new choice or create a fallback loop by choosing a
generation and a bounded number of attempts.

## Streaming responses

Before forwarding, the Responses SSE path observes bounded event data to determine the first valid
output, the terminal state, token usage, and the history result. The proxy neither buffers an
unbounded infinite stream nor turns provider text into a fallback input. Timeouts, connection
failures, upstream HTTP statuses, and protocol errors keep distinct, stable classifications.

## Chat Completions upstream compatibility

A Chat Completions route still presents the Responses contract to Codex: the request is translated
within this single attempt, the upstream response is translated back into Responses events, and
fallback, history, usage, and diagnostics keep their existing semantics. The translation is bounded
and fails closed and does not silently drop semantics:

- Tool declarations are flattened into namespaces with a 64-byte cap (a SHA-256 suffix is appended
  when too long); duplicate or empty names always fail closed instead of silently dropping a tool.
- The hosted `tool_search` uses a fixed synthetic function declaration and is restored as
  `tool_search_call` on the way back; the default-shaped `web_search` declaration (only the
  `external_web_access` boolean) is omitted and a compatibility marker is recorded, while richer
  shapes fail closed.
- Custom tools embed the raw declaration in the function description, wrap the call input as a string
  field, and are restored as `custom_tool_call` on the way back.
- Readable reasoning text is replayed into the corresponding assistant turn, and `reasoning.effort`
  values `low | medium | high | xhigh | max` map to the top-level `reasoning_effort`; only opaque
  `encrypted_content`, provider-proprietary shapes, or unsupported values fail closed.
- Codex local compaction remains a supported path; a request carrying `compaction_trigger` fails
  closed before being sent upstream.
- If the upstream ignores `stream: true` and returns JSON, the same item-building logic still
  produces Responses SSE.
- Incremental SSE framing has a 256 KiB cap: a single complete frame and an incomplete tail are both
  subject to the same cap; exceeding it emits a bounded failure for the turn and ends it rather than
  growing the buffer without bound.
- When the upstream provides no usage at all, the router does not fabricate billing data; when the
  upstream provides usage but is missing required fields, it zero-fills required fields such as
  `input_tokens_details.{cached_tokens,cache_write_tokens}` per the Codex 0.155.1 contract instead of
  failing the whole response parse. In this version, Chat routes fail closed for non-streaming client
  requests.

These compatibility behaviors affect only Chat Completions routes; Responses routes keep
byte-identical forwarding.

## Shared outbound policy

The desktop layer reads the macOS static proxy configuration and subscribes to network configuration
changes; the core layer only consumes immutable snapshots and does not touch workstation settings.
Inference, balance, model discovery, address probing, image generation and asset download, and app
updates all share this policy. Loopback is always direct; a custom proxy takes precedence over system
configuration. System mode first matches bypass rules, then selects the manual proxy or SOCKS for the
target protocol; when no applicable egress exists, it connects directly if automatic proxy is not
enabled, and fails closed if only PAC/WPAD is available. A system read/validation failure and an
unreachable selected proxy do not authorize a direct connection.

PAC/WPAD is never fetched or executed; a mixed configuration uses only the applicable manual
proxy/bypass rules and does not attempt automatic-configuration candidates or replay requests.
Environment-variable proxies do not participate in the decision. New operations adopt the latest
configuration while already-started operations keep their snapshot; both the initial target and every
redirect target must pass the policy validation, and a rejection cannot be escaped through a loopback
redirect. WebView pricing sync and session retrieval keep the platform network stack.

## Image result compatibility

The Images MCP prefers the first string-typed `b64_json` in the `data` array; only when no such field
exists does it choose the first string-typed `url`. Empty or invalid base64 is still handled as an
encoding error and does not fall back to the URL to mask the failure. Both paths must pass full PNG
validation and private publication, and success still returns a single text JSON block containing
`status / path / mimeType / width / height / bytes / sha256 / assetId`. The HTTP image entry point
keeps successful response bytes unchanged and does not parse URLs out of them or download assets.

Asset downloads use a separate credential-free client, but routing follows the shared
system/custom-proxy policy; one policy snapshot is pinned for a given download and its redirects.
Generation-API keys, gateway tokens, cookies, referrers, and environment proxies are not inherited.
A URL only needs to pass the client's parsing and request construction; there is no additional
admission on URL length, scheme, port, userinfo, fragment, host class, DNS results, public address, or
remote peer. Resource caps, deadlines, PNG integrity, and private publication still apply.

At most three explicitly handled redirects are followed, that is at most four asset GETs; the entire
download phase shares a 600-second deadline. Supported HTTP(S) targets are requested by reqwest under
the shared policy, and a target that cannot be constructed or sent is handled as an asset download
failure. The transport and the content-decoded asset are each capped at 48 MiB, while final
validation is still based on the PNG bytes.
An image source with additional authentication does not inherit generation-request credentials;
whether an image source is reachable is decided by the network outcome of the independent GET.

## Image errors

Images HTTP/MCP always calls the selected image route exactly once, does not retry automatically, and
does not enter text fallback. MCP errors explicitly distinguish request construction, connection,
send, upstream timeout, response body read, upstream HTTP status, response decoding, result
validation, asset download, and asset storage; they also return a stable local code, a local
requestId, a numeric-or-null upstreamStatus, a closed category, and retryable. retryable is only for
the caller to decide whether a later manual retry is worthwhile; it never makes the router replay
this request.

A 3xx response from the image-generation API is treated as an upstream non-success status; the
generation client explicitly disables automatic redirects and retries so that a 307/308 cannot cause
a second generation POST. Image URLs in generation results use the independent controlled GET
redirect rules described above.

When no usable image carrier is returned, the result is `image_result_missing`; a URL parse, request
construction, network, status, or response read failure returns `image_asset_download_failed` with
stage `asset_download`. These local errors use fixed descriptions, with category `unknown_upstream`
and retryable false; a download failure does not regenerate the image or switch routes.

upstreamStatus points at the actually relevant operation: for a source-selection/validation failure it
is the generation response status; when the asset URL is being parsed or the request has not yet
received a response it is null; for an asset GET or redirect-handling failure it is the relevant asset
response status. PNG validation uses the response status that supplied the image bytes. A
generation-successful 200 must not stand in for the status of a failed download.

An upstream HTTP error parses only lowercase `error.{code,message}` or top-level `{code,message}`
within 64 KiB. The category comes only from an exact, case-sensitive code allowlist; an unknown code
is always `unknown_upstream` and is never guessed from free text or the HTTP status. Only when there
is no valid code do 400/422, 401, 403, 429, and 5xx use the closed fallback classification by status.
Content-policy, parameter, authentication, permission, and quota failures are not retryable; 429 is
retryable, 500/502/503/504 are retryable, other cases follow the fixed matrix, but every branch still
keeps a single upstream call.

## Diagnostics and history

Errors returned to Codex are a bounded Responses-style DTO. The current response may show a provider
error message after length and control-character processing, but it never enters the run log, request
history, recovery points, or long-term inference state. History stores request ID, route, attempt,
status, time, tokens, cost estimate, and fallback result; it does not store prompts or full response
bodies.

Known Images categories use only fixed safe messages. Only `unknown_upstream` may append to the
current MCP message a provider message that has been cleaned of control characters,
whitespace-collapsed, and limited to 240 Unicode characters; provider codes, request IDs, headers, raw
bodies, and underlying network/IO errors never enter MCP safe fields, logs, or persistence.
Asset URLs, signature parameters, and Locations likewise never enter error descriptions or
diagnostics; a failed download HTTP error body is discarded outright.

When changing routing/fallback, prove at least:

- the snapshot stays consistent within one request;
- the retry classification and the response-commit boundary are accurate;
- the maximum number of attempts is finite and an old generation cannot be persisted;
- API keys, URLs, bodies, and provider messages do not enter logs or persistent diagnostics;
- streaming success, a disconnect before termination, and a disconnect after termination each have
  a determinate result.
