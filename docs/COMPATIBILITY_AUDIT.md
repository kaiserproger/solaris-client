# C00 compatibility and provenance audit

Audit date: 2026-10-01. Scope: the authoritative native-client specification was read in full, followed by all three reference repositories' AGENTS. References remained read-only. This is source/feasibility evidence, **not acceptance of a complete client, C02 gameplay, online authentication or performance**. NativeClient owns integrated acceptance; RenderWindow owns `.analysis/evidence/C00/gpu.json`.

## Locked target and independently available oracle

`compatibility.lock` is JSON. It records actual source digests, pinned Rust/dependency versions, reference HEADs **and individual current source-file hashes**. Dirty concurrent checkouts are not reproducible production dependencies. No dependency on a sibling dirty checkout is introduced.

| Fact | Observed value | Authority |
|---|---|---|
| Java release / protocol | 26.1.2 / 775 | `version.json` embedded in both SHA-verified official archives |
| World data version | 4790 | Same embedded metadata |
| Resource pack format | 84.0 | `resource_major/resource_minor` |
| Data pack format | 101.1 | `data_major/data_minor` |
| Java runtime requirement | 25 | Embedded metadata and descriptor |
| Current descriptor SHA-1 | `ee044d53f31fdcbfa31be3684ad321d44c81c3a5` | Fresh trusted official manifest → descriptor, digest verified |
| Client SHA-1 / bytes | `4e618f09a0c649dde3fdf829df443ce0b8831e65` / 38,113,927 | Official descriptor, actual local bytes |
| Server SHA-1 / bytes | `97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51` / 60,417,480 | Official descriptor, actual local bytes |
| Current asset index 30 SHA-1 / bytes | `2c17565f9cd7dbb95294e71fe258d65d6a8f0360` / 548,391 | Fresh descriptor → official index, digest verified |

Actual client canonical path: `/home/kaiserroman/.gradle/caches/fabric-loom/26.1.2/minecraft-client.jar`. The convenient reference path `../solaris/.analysis/client-automation/versions/26.1.2/client.jar` is a symlink; a zero-byte directory preview was not the JAR's actual size. Both that path and `../solaris/.analysis/decompile/client.jar` read the same verified bytes.

Actual vanilla server bundle: `/home/kaiserroman/solaris/.analysis/server.jar`. Actual Java: `/home/kaiserroman/.sdkman/candidates/java/25.0.4-graal/bin/java` (Java 25.0.4). Actual existing `../solaris/target/debug/wire-probe --help` ran successfully, exit 0. Oracle launch is an isolated local working directory with `java -Xms512m -Xmx2g -jar /home/kaiserroman/solaris/.analysis/server.jar nogui`; EULA acceptance and gameplay launch belong to the approved operator/integration harness, not this audit. Existing player worlds must not be written by the oracle. Availability is proven; an audit-only help run is not gameplay parity.

`javap -classpath <verified-client.jar> -p -c` actually ran for Game/Configuration/Login/Status/Handshake protocol registration classes (exit 0), and separately for nine C02 packet/data classes (exit 0). Original bytecode listings remain ignored in `C00/javap-protocols.txt` and `C00/javap-C02-layouts.txt`. The derived receipt `C00/packet-ids.json` records every state/direction's exact zero-based registration order and the local server-generated report name for that ID. Bundle registration occupies Play/clientbound ID 0; Java field names and report names need not match. Counts agree: Play 141 clientbound/69 serverbound; Configuration 20/10; Login 6/5; Status 2/2; Handshake 1 serverbound. Examples independently inspected here: Play Login 49 (0x31), chunk-with-light 45 (0x2d), container content 18, player position 72, sound 117; configuration registry data 7 and finish 3. These are evidence tables, not a second production protocol enum.

## Shared Rust crate dependency/API audit

Reference Solaris HEAD at audit: `4865344edb3733eacd5a942cdd7b548f39a80126`; source hashes in the lock account for the working tree. Candidate libraries are version 0.0.8 workspace packages; no production reuse is claimed without a clean pinned revision containing the needed APIs.

| Candidate | Actual production dependencies | Reusable boundary | Client gap / constraint |
|---|---|---|---|
| `mc-protocol` | `mc-data`, `mc-domain`, `mc-nbt`, bytes, flate2, thiserror, uuid | Public primitive codec, plaintext framing/compression, state-grouped typed packets | Does **not** depend on mc-net or mc-server. Generic Packet ID is scoped by caller state/direction; it is not a complete client state machine or dispatch table. No account OAuth/session-join implementation. |
| `mc-data` | `mc-domain`, `mc-nbt`, serde, serde_json, same-file, thiserror, tracing | Namespaced identifiers; sidecar loaders; selected block/item/collision/data facts; bounded resource-path handling | Server-oriented `Registry.entries` is lexicographically sorted, unlike vanilla registration order. Client registry IDs must preserve received session order, never assume this filesystem order. `solaris_required_data()` is a minimal server fallback, not full client content/registry proof. No client pack stack/model baker/font/audio renderer. |
| `mc-nbt` | bytes, thiserror | Bounded network and named Compound-root codec, modified UTF-8, iterative/budgeted traversal | `read_network` and `read_named` require Compound root. General text components and other root tag types need their actual packet-specific codecs; the existing network function cannot be assumed a universal NBT decoder. Limits are private production settings, not a public per-untrusted-owner adjustable budget API. |

### Concrete decode gaps and supported seams

The static `C00/mc-protocol-api-inventory.json` lists actual Packet trait implementations and source locations; presence of `decode` is not proof of arbitrary payload support or semantic packet application. Source-inspected concrete cases:

- Handshake, Status, login-success and configuration registry-data decoders exist. RegistryData parses network NBT and serializes it again into owned bytes: client use pays parse/reserialize work and cannot assume byte-identical preservation. Reconfiguration, received runtime registries, optional-known-pack resolution, transfers and stale-epoch handling remain client owner responsibilities.
- `frame.rs` explicitly says AES-128/CFB8 encryption is owned by mc-net transport, below plaintext framing. The package/lib descriptions mentioning encryption do **not** mean mc-protocol supplies it. Frame outer maximum is `(1<<21)-1`; decompressed maximum is 8 MiB. Client transition timing and aggregate queue budgets are still necessary.
- Chunk-with-light decode exists, but `LevelChunkWithLight.data` remains `Vec<u8>`. Palette/bit-packed sections, biome values, received light application, dimension height, collision neighborhood and render extraction need a real client-side consumer, not a copied server WorldStorage.
- `ItemStackWireCodec` accepts at most five added components and zero removals. Implemented component subset is damage, literal custom name, item model, enchantments, suspicious-stew effects. Other component IDs/patch shapes fail. Literal custom-name NBT accepts a single `text` string only. Enchantments resolve through Solaris's required registry index, not an arbitrary received session registry. Full component-aware containers/vanilla text remain blocked by these gaps.
- PlayerInfo decoding rejects INITIALIZE_CHAT session data and nonempty display-name components. Full signed-chat/player-list support cannot be claimed.
- Entity metadata recognizes selected serializers only; unsupported serializers fail `entity data serializer is not implemented`. This is not all vanilla entity metadata/poses/render-family coverage.
- Explosion decoding supports only selected simple particle IDs and rejects inline sound holders. Merchant exact component predicates are unsupported. SetTime only accepts known current world-clock entries. These are real client-visible restrictions, not malformed-input validation that can be called full coverage.
- No mc-net/server simulation, SessionRegistry, persistence, server Brain, or world authority is needed in the multiplayer client. A narrow owning upstream extraction is preferable to dragging server transport/auth/runtime just to reuse a kernel. The current new client does not link speculative empty crates or claim these candidates are complete.

## Exact Loader/native platform gate

Loader HEAD at audit: `bab2329c707d4bff026f22b65162e93b432ab50c`; plugins HEAD: `9adf5aebceb7595ca75f8e93ad82ef01739d328a`. Current Java Loader and Rust server agree: **wire 3 / artifact index schema 2 / only fabric, neoforge, forge**. Native cannot legitimately impersonate Fabric.

`LoaderHandshake` rejects unknown manifest/bundle fields, requires protocol 3, known enum values, nonempty unique lists, explicit platform support, content-to-permission correspondence and exact `owner:id/version/sha256` cache keys. Manifest fields: `protocol,bundles`; bundle fields: `owner,id,version,artifact,sha256,size_bytes,loaders,content,permissions,cache_key`. ACK fields: `protocol,platform,loader_version,accepted_permissions,cached_bundles,carrier_block_state_ids`.

Index path is `solaris-client.json`; allowed fields `schema,screens,world_previews,blocks,items,assets,sounds`. Eight widgets are `paged_table,tabs,input_number,input_text,select_enum,resource_panel,action_button,world_marker`. Content kinds map respectively to registration, view, view-action, asset, world-preview, world-selection and sound permissions. Namespaced view keys are exact identifiers; a product enum such as settlement/army is not the shared view contract.

Observed limits: manifest 32,767 B; index 64 KiB; bundle 64 MiB; screens 64; previews 64; blocks per bundle 1; activated blocks 8; items 128; assets 128; sounds 64; activated asset bytes 64 MiB. Native is not permission to increase capacities. Consent occurs before transfer; controller generation fences stale prompts; refusal does not start transfer. Java acknowledgement is constructed from activated content, including sorted block-to-carrier mapping. Cached ZIP bytes alone are not activation.

The actionable owning contract report is `.analysis/upstream/native-platform-request.json`: honest `native` identity, explicitly versioned capability negotiation and platform-filtered portable bundles while old Java wire3 remains unchanged. Revision/channel allocation is deliberately not invented. Closed wire3 cannot acquire arbitrary capability fields silently. Required owning changes and positive/negative acceptance are specified. No upstream source was edited; **Solaris-native acceptance remains externally blocked**. Existing `solaris:plugin@0.7.0` is server-only; no client-WASM ABI exists merely by naming `solaris:client`.

The C02 refresh compiled the current read-only Java declarative MCP catalog and executed its real `definitions()/toJson()` export: **47 tools**, adding on-demand `minecraft_read_item_components` and exact native `minecraft_set_screen_input`; the namespaced view request was already present in the accepted 45-tool baseline. `ClientMcpTools.java` SHA-256 is `fd15166a59c62769b4ecc9473c622b9c5ee03ca55ddc18acdfb38c0bb0f180ec` (29,940 B). The complete refreshed schemas/annotations/command/execution and 47 explicit native gaps are in `fixtures/mcp/loader-catalog.json`; the original C00 bytes remain `.analysis/evidence/C00/loader-catalog-baseline.json`. This is source/export proof, not execution of Loader runtime controls or native Loader compatibility. The native preview still exposes 13 tools.

The new component query reads the actual selected/player/visible-container stack, uses native registry-aware JSON component codecs, preserves custom data as `{"snbt":"..."}`, and refuses unknown/non-serializable components rather than silently omitting them. The EditBox control requires exact screen class/title and unique visible enabled editable label, validates the actual maximum length, then focuses and invokes native `setValue`/responder; stale/truncating controls fail. Neither contract is implemented by the Rust preview or replaced with a direct model/packet shortcut. Concurrent Loader changes do not waive the unchanged wire3/schema2 native-platform gate.

## Actual assets, inventory and provenance boundary

Fresh official manifest, descriptor and asset index GETs succeeded. Descriptor/index digests and exact sizes were checked; local client/server archives match official download digest/size. Actual JAR inventory: 10,463 `assets/` entries, 5,520,593 uncompressed B. Categories include 3,833 texture entries, 3,676 models, 1,506 item definitions, 1,170 blockstates, 106 particles, 88 shader files, 45 equipment resources, 15 atlases and 7 font definitions. Index contains 4,750 objects totaling 456,619,436 reference bytes. **All 4,750 current objects match the current official SHA-1 and size**, with additional observed SHA-256.

The current official version metadata differs from the cached descriptor: client/server unchanged, 142 language objects changed. Historical descriptor/index SHA-1s were `0d0f7a851642f3c81d1cff0a608aa048e5e8319f` / `25c0d18dd5ab0e3a7515f3704f401de547da4b1c`. Both local player `30.json` files had the same byte length but different digests from the pinned official index: size/index ID is insufficient provenance. We fetched and pinned the current trusted chain, imported 4,608 matching objects read-only, and downloaded 142 missing objects (89,620,564 B) under ignored C00 storage. Existing player caches were not altered.

Current owned root: `.analysis/evidence/C00/assets`, with `objects/` and `indexes/30.json`; current index: `.analysis/evidence/C00/asset-index-current.json`. The 4,608 imported objects are symlinks to verified local objects, not source-tree copies. Parent separately safely extracted verified JAR `assets/` under `.analysis/resources` with traversal/duplicate/size checks; its runtime resource consumer and ASSET acceptance remain parent-owned.

Public receipts `fixtures/content/source-inventory.json` and `source-inventory-historical.json` contain **names, sizes and digests only**, not any original JSON/resource payload. Local exact source paths and download drift are in ignored C00 receipts. Archive inventory is not the entire behavioral implementation: entity rigs/procedural animation/shape rules may be Java-defined. Finding JSON/PNG does not establish implementation. Mojang GLSL shaders are inventoried, not copied into WGSL or claimed compatible.

The authored redistributable fixture under `fixtures/content/` actually contains opaque/cutout RGBA PNGs, a standalone authored six-face model, synthesized PCM16 WAV and Vorbis OGG, `sounds.json`, `pack.mcmeta` and per-file hashes. `generate.py` has no Mojang input. License is MIT OR Apache-2.0; generation uses mathematical waveform/checker expressions. OGG byte reproducibility requires the same ffmpeg/libvorbis build, explicitly recorded in provenance. This is useful fixture content, not a substitute for vanilla asset completeness. Actual Vorbis probe: mono, 48 kHz, duration 0.250000 s.

[Minecraft EULA](https://www.minecraft.net/en-us/eula) explicitly prohibits distributing Mojang-created software/content absent permission. Official CDN availability and matching hashes are technical provenance, **not legal permission for independent client distribution**. No JARs, Mojang textures/models/fonts/audio/shaders, decompiled sources or caches derived from their bytes may enter Git, binary releases, containers or public CI. Distribution requires a separate licensing review. Original authored fixtures and own source code are the release-safe boundary. This research is not claimed clean-room. No Sodium/Nvidium/Prism source was copied; upstream license and algorithm provenance review remains mandatory before borrowing implementation.

## Authentication feasibility: independent external blocker

`C00/auth-feasibility.json` records the absent own public-client configuration. No foreign application ID, password, token, borrowed launcher secret or account credential was read/used. No account sign-in, entitlement or session join was attempted. Public Microsoft consumers discovery GET succeeded HTTP 200; this only proves public endpoint reachability.

[Microsoft native authorization-code guidance](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow) documents system browser, registered localhost redirect and PKCE S256; public native clients must not embed secrets. [Application registration](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app) requires an owning registered client ID and appropriate personal-account audience. [Xbox website documentation](https://learn.microsoft.com/en-us/gaming/gdk/docs/services/fundamentals/s2s-auth-calls/service-authentication/live-website-authentication) documents consumers authorize/token, Xbox user authentication and XSTS endpoints, scopes and audience requirements; its confidential-website example is **not proof** of permitted public-native Minecraft access.

Missing external prerequisite: the owner's own application registration/redirect/audience, provider-approved Xbox/Minecraft access and an interactively authorized entitled account. Xbox relying party/scopes and Minecraft-specific exchange/profile/entitlement/session-join success must be independently validated for that own app. These remain blocked, not silently inferred from standard OAuth documentation. Device flow is not assumed permitted. Tokens must eventually use OS secure storage or explicit nonpersistent mode, never CLI/log/MCP/plaintext persistence. An offline/test identity is permitted only in the explicit local test profile and closes no online-auth gate. Asset, renderer and automation development continues independently.

## Stack and measurement boundaries

Actual toolchain pin: Rust 1.95.0; package declared MSRV 1.88 (not separately tested). Exact direct versions/features are in Cargo.toml; resolved sources/checksums in Cargo.lock. wgpu 27.0.1 metadata declares MSRV 1.88, winit 0.30.12 MSRV 1.70.0, rodio 0.21.1 audio backend. Direct dependency license strings/MSRV/metadata digests were read from the actual fetched crate manifests and recorded in the lock; winit is Apache-2.0, bytemuck permits Zlib/Apache/MIT, other exact strings are retained rather than normalized. Full transitive license/platform/MSRV release verification remains separate.

Final direct dependency consumers additionally pin SHA-1 0.10.6 for original asset-object verification and base64 0.22.1 for bounded PNG resource blobs. Rodio default features are disabled; only `playback,vorbis` are selected, not unused MP3/MP4/FLAC/WAV decoders. The current fetched lock resolves 318 packages including the local package; this is a dependency-resolution result, not a build pass.

Parent-observed local feasibility: X11 display `:1`; Java 25.0.4; PipeWire 1.0.5; real ALSA analog stereo sink; ALSA 1.2.11 local build-support extraction, not a system mutation. These are **parent observations**, not this worker's audio capture. GPU enumeration/device request, supported features/limits and graphical presentation require actual RenderWindow receipt. Hardware/TGP/driver/compositor/refresh/cooling reference performance profile is not frozen by a guessed laptop model. Offscreen GPU is not native window evidence; device enumeration is not FPS.

`fixtures/performance/scenarios.json` preserves P01–P10 original workloads and development targets, fixed 1080p/Parity/view16/normal-particles equality, five ten-minute steady repeats after identical warmup, cold streaming separation, raw samples including skipped/worst frames, and exact required scene/camera/input/visible-set/hash/hardware/settings inputs. P03 retains 1,024 fighters plus 50 avatars; P04 retains 4,096 fighters; P05 is 100k **received records** with a fixed 4,096 visible-eligible subset, not 100k drawn humanoids. Memory/soak/MCP/edit-latency gates are unchanged. Adversarial geometry/transparency/material/block-entity/light/inventory/reload/font/packet and lifecycle-race descriptors preserve the specification's scope. Execution readiness is honestly false until representative immutable recordings and matching comparators exist; `measurements:null` is intentional. No FPS, timing, equality seed, visible-set or passing result was fabricated.

## Evidence index and acceptance responsibility

All original bytes/raw source listings are ignored under `.analysis/evidence/C00/`:

- `source-receipts.json`: historical verified chain/source receipts; historical is explicitly separate from current lock.
- `manifest-fetched.json`, `descriptor-current.json`, `asset-index-current.json`: current trusted-chain original metadata.
- `source-receipts-current.json`, `compatibility-lock-smoke.json`: current manifest/descriptor/index chain, exact source digests, and an actual consumer reading/verifying all 4,750 owned current objects.
- `metadata-drift.json`, `asset-object-current-local-paths.json`: exact current completeness and imported/downloaded source split.
- `packet-ids.json`, `javap-protocols.txt`, `javap-C02-layouts.txt`: actual packet source evidence.
- `mc-protocol-api-inventory.json`: static API availability, not a runtime semantic pass.
- `oracle-availability.json`, `wire-probe-help.txt`: real Java/javap/probe availability; no audit gameplay claim.
- `public-metadata-fetch.json`, `auth-feasibility.json`: actual public GETs and independent own-application blocker.
- `fixture-smoke.json`: actual media decode/shape/hash smoke receipt.
- `gpu.json`: reserved RenderWindow runtime result; parent controls C00/C01 acceptance.

No builds, formatters, linters or project-wide test suites were run by this worker. Source audit, official metadata/object verification, real javap/help execution and authored media generation/probe are exercised evidence only. Final Cargo/runtime/gameplay/UI/audio/window acceptance and independent review belong to NativeClient. Remaining external gates are own-app official-service approval, upstream native Loader contract approval and pre-release legal review; full client completeness/performance remain implementation and acceptance work, not external excuses.
