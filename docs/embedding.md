# Embedding guide

A native bundle is the integration unit. Keep its libraries together and use its own generated `zeb_game.h`, `manifest.json` and `consumer.c`. Export names include a build identity; do not invent names or reuse a header from another bundle.

## Scalar bundles

Scalar `main` accepts zero to three signed integer arguments. The generated header states the actual signature. The zero-argument entry uses ABI version 1; entries with integer arguments use version 2. Results are packed into a 64-bit word: the low 32 bits carry a tag, the high 32 bits carry an integer payload or source offset. Tags 0, 1 and 2 represent nil, true and integer. Error tags are decoded by the supplied consumer; do not treat an error word as a successful result.

`examples/embedding.c` calls a one-argument entry with 20 and checks the result is 41. Its Python build helper reads the symbol and library names from the manifest and invokes the selected Clang and SDK. The generated consumer demonstrates argument parsing and error reporting for every scalar signature.

## World bundles

The current world profile is `object-shared-v12`. The manifest is the source of truth for its ABI number, symbols, entry points, entities, relations, enums, export signatures and inspection operations. The Rust dylib's internal types and mangled names are implementation details.

The session boundary is scalar and pointer-free. Its generated exports cover:

- Preparing a session slot with object/property/output limits and starting its worker.
- Polling requests and output, draining text and semantic events, supplying text bytes, action IDs, subjects or values, and resuming the worker.
- Inspecting supported state at safe boundaries and reading the resulting words.
- Resetting, finishing and closing sessions and retrieving their outcomes.

Use the generated consumer as the executable protocol example. Observe poll/request states before replying; acknowledge drained output before resuming. Keep session slots and entity handles separate. Handles are checked identities, not addresses, and should not be kept across reset/restore as if they were timeless. Multiple sessions have distinct slots and independent state.

An application owns rendering, input mapping, audio, networking and files. The runtime owns language state. Text is one possible interface; `act(verb, subjects)` supports applications that never parse commands. Events contain semantic IDs and related entity identities so hosts can choose their own presentation.

## Save, restore and recovery

Logical save version 1 stores supported World state at a command boundary, including supported properties, strings, collections, dictionary data and relations. It does not serialize Rust memory, active frames or arbitrary native resources. Save data is tied to the compatible program/schema identity; validation precedes replacement of live state. A rejected restore must leave the existing state intact.

The example consumer accepts `:save PATH` and `:restore PATH`. Its simple file writes are not a production atomic-publication or durability guarantee. A production host must define safe file replacement, persistence errors and storage permissions. Restoring a logical snapshot starts without the previous undo history. Native resources require an explicit recreation policy and cannot be assumed to have survived serialization.

A source exception is an explicit language outcome. Recoverable turn failure rolls back supported mutations and allows the session to continue. Resource refusals and terminal session errors are distinct; inspect the outcome rather than retrying an invalid session blindly.

## Deployment

Both architecture slices are built, and universal libraries/consumers are assembled from them. Package the manifest-selected runtime and game libraries with the host and preserve their relative loader paths. The runtime artifact can be reused between compatible program builds; generated cache keys include the toolchain and build options. A loaded runtime image installs one grammar/literal table set: concurrent sessions using different program tables require separate runtime images or processes. Cache entries are copied and checked, so deleting a build cache does not invalidate an existing bundle.

Generated build logs and tool fingerprints may contain local paths. They are local build diagnostics, not files to publish automatically. Audit any binary distribution separately for debug paths, SDK/tool licences and target execution coverage.
