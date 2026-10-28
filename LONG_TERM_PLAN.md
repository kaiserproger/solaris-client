# Long-term plan

## Sole live work cursor

```yaml
stage: P0-agent-test-client
state: in_progress
owner: Main
next: close native entity combat/drop runtime coverage on the unchanged shared profile
last_accepted_stage: C02
last_acceptance_receipt: .analysis/evidence/C02/acceptance-receipt.json
```

Only the integration owner advances acceptance after actual evidence and independent read-only review. This is the only live cursor: stage descriptions below are requirements, not parallel work cursors or completion claims. C00–C02 remain accepted; the owner-directed current implementation priority is P0 followed by C05. C03–C10 remain mandatory end-product scope, never silently dropped. External auth/service blockers do not stop independent renderer/MCP work. A preview must state its exact boundary; C00–C03 alone are not a complete client.

Owner-directed priority is P0 agent-test client first, then C05. The consolidated state-only native binary passes shared survival end-to-end (snow clearance, dirt mining/pickup, hotbar selection, placement and consumption) and the shared inventory/crafting profile. Resource-free startup is verified without `RESOURCE_ROOT`, `ASSET_ROOT`, or `ASSET_INDEX`; protocol/gameplay metadata paths remain required. Latest single-core survival receipt: `/home/kaiserroman/solaris-harnessnative/.analysis/validation/20261002T102113-survival-s693n23l/result.json`; inventory receipt: `/home/kaiserroman/solaris-harnessnative/.analysis/validation/20261002T102516-inventory-h8zuddvl/result.json`.

State mode does not load resource packs, localization, fonts, atlases, render rigs, terrain render-face caches, or neighbor-face suppression tables. Heaptrack identified full placement/terrain JSON trees and a placement-table clone as the dominant startup consumers; typed semantic decoding and early source-tree release reduced clean single-core survival peak RSS from 644692 to 205400 KiB. Heap peak fell from 462.04 to 123.76 MB; profiling receipts/stacks are retained under `.analysis/native5/`. The single clean before/after pair is memory evidence, not a stable wall-time performance claim.

The previous adjacent-snow failure was premature lifecycle completion during the ordinary destroy delay. Headless inventory failures were stale pre-synchronization UI contexts and screen publication that depended on rendering; synchronization now follows ordinary inventory effects and received-state ticks. Entity listing, visible wait, approach, pig feeding, and one received-health-confirmed attack have runtime evidence; repeated combat/drop collection remains under verification on the unchanged shared profile. This is not full-client acceptance. C03 scale/performance and remaining C04 completeness remain deferred without acceptance or scope deletion. C00–C02 acceptance is unchanged.

The 2026-10-28 protocol-777 rc.1 cutover is a **scoped compatibility result**,
not a stage advancement: all 255 Login/Configuration/Play IDs match the explicit
26.3 report; changed component/advancement/entity codecs have authored exact-byte
regressions, and all 161 entity schemas use verified 26.3 facts. The r4-pinned
client passes the real-window native lifecycle plus ordinary axe stripping and
hoe tilling, reconnect persistence, 64 grounded settling ticks without
rubber-band, a known-lit slab/stair/leaf/wall capture and unsignalled normal Quit.
Exact binary/server identities, fixtures, review and receipts are linked from
`docs/AGENT_TESTING.md`. Xvfb/lavapipe output is not physical scanout or a vanilla
AO oracle; zero-soak is not long-session evidence. Commands/other explicitly
unsupported presentation and the four added entity visual families remain
unaccepted, and inherited lint/GPU-environment issues remain in `BUGS.md`.
C00–C02 acceptance and the sole P0 cursor above are intentionally unchanged.

## Gate and evidence policy

Each stage supplies working vertical behavior. Unit tests, prepared fixtures and a window alone do not substitute for gameplay, GPU/audio or service evidence. Preserve source workload/settings after A/B starts. Parent records exact tree/build/locks, platform/device/backend, server/oracle, input and packet/event traces, assertion results, raw metrics, frame/resource/state IDs and actual audio sink. Native onscreen evidence is required where specified. No accepted row without a receipt. Unavailable external prerequisites stay explicit and prohibit the associated completeness claim.

C00, C01 and C02 have accepted parent receipts and independent read-only reviews. C02 includes both official-server and Solaris ordinary survival movement/mining/pickup/hotbar/placement, authoritative inventory, native/GPU frames and isolated output PCM evidence. The Solaris global-remesh timeout is retained, and the chunk-local cache fix passed the same real input path without increasing its deadline. Initial C02 review findings for solid-cell light sampling and a lost saturated-queue terminal error are also retained with failing-before/passing-after regressions and corrected-binary real-window/Configuration-EOF smoke. The independent rereview found no remaining actionable defect in that C02 scope. Main explicitly authorized native-only temporary null-sink capture instead of hardware audibility on the muted late-night laptop; physical sink volume/mute remain untouched, and hardware-audibility completeness is not established. This acceptance does not claim the full survival/audio scenarios were repeated on the corrected binary or close any C03–C10/domain completeness gate.

The following stage scope and exit/acceptance wording is retained verbatim from the authority. Each source range maps to stable `SRC-NNNN` IDs in [SPECIFICATION.md](SPECIFICATION.md). C10's final gates also include §28 and every applicable §23 acceptance row, not just the stage paragraph.

## C00 — контракт, provenance и измерительный стенд

Source: original lines 945–950 (`SRC-0945`–`SRC-0950`).

Зафиксировать target/versions, licensing boundary, доступ собственного auth приложения, source asset inventory, gap analysis `mc-protocol` и Loader, shared MCP fixtures. Выбрать и закрепить stack; написать минимальный compatibility matrix и perf scenario descriptors. Измерить hardware/capabilities. Подготовить authored fixture pack и независимый vanilla oracle path.

**Выход:** кодовые spikes отвечают на auth/asset/render/MCP feasibility; чёткий список внешних blockers; нет runtime skeleton из десятков пустых crates. Не считать недоступность auth API причиной остановить независимый renderer/MCP fixture development.

## C01 — реальное окно, render loop и MCP до игровой логики

Source: original lines 951–956 (`SRC-0951`–`SRC-0956`).

Window lifecycle, device/capability selection, inputs, basic render scene собственного fixture, UI semantics, screenshot, metrics, instance discovery, localhost/stdio MCP и controller safety. Schema/version adapters тестируются отдельно.

**Приёмка:** реальный GPU кадр, resize/focus, input/cancel, authenticated tool discovery, capture. Headless-only не закрывает C01.

## C02 — настоящий vanilla session и первый gameplay loop

Source: original lines 957–962 (`SRC-0957`–`SRC-0962`).

Status/login/config/Play, chunk decode/light, камеры, player prediction, базовые модели/texture resources, hotbar/inventory, break/place и звук. Подключение к официальному target server и Solaris. Обычный игровой survival сценарий: подойти, добыть блок, собрать, использовать, сохранить session correctness.

**Приёмка:** ordinary inputs через MCP; server-observed inventory; actual frame/audio; никаких тайных `give/teleport` для закрытия пользовательского пути. Любой fixture setup обозначен отдельно.

## C03 — масштаб terrain без потери картинки

Source: original lines 963–968 (`SRC-0963`–`SRC-0968`).

Bounded streaming, incremental meshing, model baker coverage, culling/instancing baseline, GPU arenas, resource reload и репрезентативная прозрачность. CPU/GPU/frame counters. Первые P01/P02 A/B и correctness reference renderer checks.

**Приёмка:** не только ровный мир: пещеры, растительность, вода, города, edit bursts, teleport и high-res pack memory failures.

## C04 — entities, все взаимодействия, containers и UI

Source: original lines 969–974 (`SRC-0969`–`SRC-0974`).

Последовательно закрыть full matrix видов entities/block entities, poses/equipment, vehicles, combat presentation, all inventory modes/screens, effects, chat/text/accessibility, audio variants. Пакетные families и auto-enumeration не заменяют ручную проверку сложных исключений.

**Приёмка:** все обязательные функции target имеют coverage; placeholders и unsupported явно блокируют соответствующую completeness строку. Толпы и P03 уже измеряются.

## C05 — Solaris native compatibility

Source: original lines 975–980 (`SRC-0975`–`SRC-0980`).

Утвердить native platform/capabilities, перенести Loader codec semantics на Rust, shared test fixtures, exact permissions/activation, generic namespaced UI, custom content mapping и server disconnect cleanup. Не создавать feature shortcuts только для settlements.

**Приёмка:** реальные standard addon scenarios, construction preview, warehouse UI, army commands через UI, звук и dynamic content на том же server build, что Java+Loader oracle.

## C06 — полноценный client-WASM host

Source: original lines 981–986 (`SRC-0981`–`SRC-0986`).

Отдельный WIT, permissions, event subscriptions, bounded host commands, resource quotas/lifecycle, error isolation, optional client storage. Выпустить representative open-source client component; server-side modules остаются серверными.

**Приёмка:** полезное расширение UI/animation + adversarial components; нет render-thread stalls и скрытых полномочий. Client-WASM невозможно включить только по требованию вредоносного сервера без local consent.

## C07 — GPU-driven fast path и большой бой

Source: original lines 987–992 (`SRC-0987`–`SRC-0992`).

Indirect submission/compaction, compact geometry/instances, shared skinning и relevant optimizations по профилю. Full P01–P05, correctness under same image settings. Затем optional mesh-shader/native backend spike.

**Приёмка:** измеренные CPU/GPU/VRAM улучшения, одинаковые scenes, отсутствие visual regression. Не принимается fast path, который работает только на одной видеокарте и ломает базовый renderer.

## C08 — singleplayer, services и desktop completeness

Source: original lines 993–998 (`SRC-0993`–`SRC-0998`).

Управление local Solaris worlds, backup/import, pause/LAN, supported official services, полнота screenshots/audio/settings/input на Windows/Linux/macOS. Закрыть внешние API blockers либо сохранить соответствующий статус и не объявлять сервисную полноту.

**Приёмка:** Java-client-compatible и Solaris-native claims основаны на матрице; native singleplayer claims ограничены фактической server parity, без скрытой JVM.

## C09 — agents, reliability, security, distribution

Source: original lines 999–1004 (`SRC-0999`–`SRC-1004`).

Полный MCP catalog и guides, migration существующих harness consumers, race/fuzz/adversarial/soak, evidence redaction, signed reproducible distribution, clean install, upgrade/rollback. Проверить минимум две независимые MCP host implementations, выбранные пользователем/проектом, а не только свой тестовый JSON sender.

**Приёмка:** независимый агент поднимает test profile по документации, проходит gameplay+Solaris UI, диагностирует преднамеренно сломанный shader/resource/state и сохраняет доказательства. Full workload повторяется на approved hardware.

## C10 — конечная приёмка и дальнейшие эксперименты

Source: original lines 1005–1009 (`SRC-1005`–`SRC-1009`).

Полная gameplay/UI/entity/resource matrix, независимый oracle, публичный список поддержанных versions/platforms/services, perf raw reports и пользовательская сессия без debug hooks. Optional LOD/RTX-like enhancements/shaderpack adapters рассматриваются после того, как имеют собственный quality contract; не требуют ухудшить Parity.

Клиент не объявлять законченным из-за закрытых C00–C03. Отдельные готовые этапы можно выпускать как preview с точной границей функциональности.

## Dependency boundaries

C00 precedes reliable pins and feasibility claims; C01 must have native GPU/window/control evidence; C02 must use actual target/Solaris gameplay and observed inventory/frame/audio. C03/C04 complete terrain and entity/UI breadth; C05 negotiates Solaris contracts without fake Fabric identity; C06 is a separate consented client-WASM ABI; C07 optimizes identical scenes; C08 bounds singleplayer/services/platform claims; C09 independently exercises agents/security/distribution; C10 closes all applicable final gates. Passing a later isolated experiment does not waive an earlier missing gate.

## Read-only reference cursors inspected

- `../solaris/AGENTS.md` and `../solaris/docs/MEMORY.md:1–65`: current server architecture/parallel tracks, draft status, no commit/push authorization; server plan remains untouched.
- `../solaris-loader/AGENTS.md:1–42`: wire 3/schema 2 owning boundaries; this document supplies rules, no separate live cursor route.
- `../solaris-default-plugins/AGENTS.md:1–22`: server component API 0.7.0 and package ownership; no separate live cursor route.

Current reference code and receipts override historical source descriptions. These trees are read-only and dirty changes are preserved. Compatibility source research/lock and protocol/Loader/MCP audit evidence are owned by integration/research agents, not asserted by this document.
