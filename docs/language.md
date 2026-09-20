# Language guide

Zebulon uses a C-like statement syntax and object declarations. This guide describes the implemented core; it is not a promise of complete TADS compatibility. Use `zebc check FILE` for source diagnostics and `zebc capabilities --format json` for the compiler's machine-readable capability profile.

## Values and control flow

The core includes `nil`, `true`, checked signed 32-bit integers, strings, object identities, lists, vectors, lookup tables, string buffers and function values. Integer overflow, invalid arithmetic and incompatible operand types are source outcomes rather than Rust panics. Collection indexes use the language's one-based convention.

Functions use `name(parameters) { statements }`. Locals use `local`; statements include conditional branches, loops, `switch`, `break`, `continue`, `return`, labelled control flow, supported `goto` forms and source exceptions. Function arguments evaluate right to left; assignment and compound-update order are covered by regression tests.

Single-quoted strings are values. Double-quoted expressions emit text. `<<expression>>` interpolates values. Files may include other source files; `--include-dir` adds search directories. No external TADS headers are required by the examples or test suite.

## Entities and relations

An entity declaration names an object; classes provide shared properties and methods:

```text
class Instrument: object name = 'instrument' reading = 0;
bench: object name = 'bench';
probe: Instrument name = 'probe';
```

A relation provides linking storage with checked identities:

```text
relation contains(container: Entity, item: Entity) one_to_many reverse location;
```

Use `contains.set(bench, probe)`, `contains.unset(bench, probe)`, `contains.contains(bench, probe)`, `contains.all(bench)` and `location.get(probe)`. A declaration can state a row as `location(bench)`. An ordinary `location = value` remains an ordinary property assignment; it does not declare a row.

Relations can have a third, enum-labelled column, and text-valued vocabulary relations integrate with dictionary and grammar matching. Relation cardinality, inverse access and label families are checked by the compiler/runtime. `modify` extends an existing declaration; generated private names do not become public entity names in a bundle manifest.

## Lifetimes and execution

The default `--model lifetimes` uses Turn and World storage, automatic promotion and deterministic cleanup. Persistent World records survive completed command cycles; temporary Turn storage is reclaimed at the boundary. Relationships use checked identities rather than shared object ownership. These language rules are implemented explicitly and are not consequences of Rust's borrow checker.

A world declares `turn(tokens)` for text input, `act(verb, subjects)` for structured actions, or both. `startup()` is optional. Declare `enum token tokWord;` for tokenized input. The host starts and advances the cycle; the program does not need to write an input loop.

A failed turn rolls back supported journalled mutations. An optional `recover(code)` reports recovery. `undo()` restores a completed interval; `savepoint()` creates an explicit interval. The implementation currently retains at most 32 intervals. Mutation undo does not promise complete reversal of entity creation or despawning.

`event(id)` and `eventSubject(entity)` emit semantic events alongside text. The host can inspect supported entity/property/relation state at command boundaries without evaluating arbitrary game code.

The explicit `--model ownership` profile provides lexical ownership and acyclic owner transfers using forms such as `local owned`, `owned` returns and `move`. It also supplies the scalar `main` entry profile. The default world model and ownership profile have distinct source lifetime rules; one is not a replacement spelling for the other.

## Compiler output and limits

`check` validates the supported source subset. `inspect` exposes tokens, preprocessed text, syntax, intermediate representation and LLVM stages. `build` supports scalar executables, objects, static libraries and shared bundles; object/world programs currently use shared bundles. `--opt O0|O2` controls optimization. Full LTO is available for optimized scalar shared bundles; it is not a general world-program option.

The default source-size limit is 16 MiB and can be set with `--max-source-bytes`. Encoding options include UTF-8, ASCII, Latin-1 and UTF-16. Diagnostics identify source coordinates, including included files where the preprocessing map applies.

Implemented facilities include a safe-Rust regex subset, dictionaries and static grammar matching. BigNumber, Date, byte packing and full dynamic grammar are not complete facilities. Resource limits, source debugging, full lifecycle reversibility and broader platform support remain incomplete. The public C surface is generated and versioned; private Rust layouts are not a portable ABI.
