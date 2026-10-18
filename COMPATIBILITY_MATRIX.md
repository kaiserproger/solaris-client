# Compatibility and acceptance matrix

Target: Minecraft Java Edition **26.1.2**. Protocol number/layouts must come from local version-specific wire-probe/javap evidence and `compatibility.lock`, never memory. Proposed spec API/CLI versions are not existence claims.

Rows begin `not_started`; integration updates only against exact receipts. C00/C01/C02 stage gates below now have exercised parent receipts and independent read-only review; broader domain and completeness rows are not implicitly accepted by those gates. Status progression is `not_started → implemented → oracle_verified → integrated → accepted`; `unsupported` is an honest development condition but cannot close a mandatory gate. Record supported variants, test/scenario IDs, oracle, platform/backend, target build/settings, source IDs and artifact links on every promotion.

This matrix includes all §4 mandatory domains, §18 automation groups, §23 acceptance IDs, P01–P10, stage gates, platform/service/server/out-of-scope rows. The full line-level register in [SPECIFICATION.md](SPECIFICATION.md) remains mandatory for detailed subfeatures and every other clause. Enumerate actual version-specific entity/container/model/packet families in audit receipts before claiming completeness; family grouping never waives difficult exceptions.

| Class | Stable/semantic ID | Required coverage or declared boundary | Authority | Status | Variants / test IDs / evidence |
|---|---|---|---|---|---|
| level | `ROW-0038` | `Java-client-compatible`: Все обязательные клиентские функции закреплённой версии, а не только несколько серверных сценариев | `SRC-0038` (line 38) | not_started | — |
| level | `ROW-0039` | `Solaris-native`: Совместимые Loader-контракты, permissions, bundles, интерфейсы, предметы/блоки/звук; новые возможности только через negotiated extension | `SRC-0039` (line 39) | not_started | — |
| level | `ROW-0040` | `Client-WASM`: Самостоятельный ограниченный клиентский ABI и полноценный lifecycle безопасных расширений | `SRC-0040` (line 40) | not_started | — |
| level | `ROW-0041` | `Singleplayer-Solaris`: Локальный Rust-сервер Solaris, управление миром, сохранение, пауза/LAN и явно выбранные правила | `SRC-0041` (line 41) | not_started | — |
| level | `ROW-0042` | `Singleplayer-vanilla-parity`: Дополнительно подтверждена полнота серверной симуляции и worldgen; успех клиента этого не доказывает | `SRC-0042` (line 42) | not_started | — |
| level | `ROW-0043` | `Official-services`: Реально проверены доступные официальные службы, включая нужные права приложения; неподдержанное явно указано | `SRC-0043` (line 43) | not_started | — |
| architecture | `ROW-0057` | Rust + `winit` + собственный renderer поверх `wgpu`: Контролируемые данные/кадр и portable GPU backend. Не писать заново Vulkan/Metal/DX12 до обнаружения конкретного ограничителя | `SRC-0057` (line 57) | not_started | — |
| architecture | `ROW-0058` | Небольшое явное client-core вместо полного игрового движка: Мир и protocol lifecycle специфичны; не покупать удобство general-purpose engine ценой двух моделей состояния | `SRC-0058` (line 58) | not_started | — |
| architecture | `ROW-0059` | Авторитетные packet-state и локальная prediction раздельны: Предсказанное разрушение/перенос предмета не считается подтверждённым серверным результатом | `SRC-0059` (line 59) | not_started | — |
| architecture | `ROW-0060` | Главный поток обслуживает окно; session owner обслуживает игру; render owner владеет GPU: Долгая распаковка, remeshing, WASM и сеть не блокируют ввод/кадр | `SRC-0060` (line 60) | not_started | — |
| architecture | `ROW-0061` | Оптимизация данных и количества работы раньше SIMD: Rust сам по себе не устраняет лишние обходы, draw calls, allocations и синхронизации | `SRC-0061` (line 61) | not_started | — |
| architecture | `ROW-0062` | GPU-driven drawing как приоритетный fast path: Не требует одного CPU draw на сущность; baseline остаётся на той же модели данных | `SRC-0062` (line 62) | not_started | — |
| architecture | `ROW-0063` | MCP использует обычный input/action path: Тест проверяет настоящий клиент; отдельного «бот-клиента с похожими функциями» нет | `SRC-0063` (line 63) | not_started | — |
| architecture | `ROW-0064` | Client-WASM отдельно от server `solaris:plugin`: Серверная экономика/инвентарь/боевые решения не исполняются повторно на машине игрока | `SRC-0064` (line 64) | not_started | — |
| architecture | `ROW-0065` | Исходные ассеты отдельно от производных caches: Проверяемое происхождение, отсутствие копий Mojang в релизе, быстрый повторный запуск | `SRC-0065` (line 65) | not_started | — |
| architecture | `ROW-0066` | Полнота и быстродействие принимаются независимыми gates: Быстрые кубы не заменяют воду, прозрачность, мобов, шрифты, инвентарь и звук | `SRC-0066` (line 66) | not_started | — |
| domain | `ROW-0126` | Launcher/shell: Профили, account lifecycle, список серверов, адреса IPv4/IPv6/DNS, необходимые SRV-правила, direct connect, progress, cancel/retry, понятные disconnects | `SRC-0126` (line 126) | not_started | — |
| domain | `ROW-0127` | Connection: Status, login, online session, encryption/compression, configuration, registries/tags, resource packs, Play, reconfiguration, transfers/cookies там, где определены выбранной версией | `SRC-0127` (line 127) | not_started | — |
| domain | `ROW-0128` | Player: Ходьба, sprint/sneak/jump, crouch/swim/crawl, вода/лава, лестницы, лед/slime/honey/soul blocks, elytra, vehicles/mounts, flight/creative/spectator, knockback, collisions, camera | `SRC-0128` (line 128) | not_started | — |
| domain | `ROW-0129` | Interaction: Raycast, breaking stages, placement, use/use-on/use-entity, main/off-hand, entity attacks, cooldowns, bows/crossbows/fishing, food, buckets, shields, reach и block/entity shapes | `SRC-0129` (line 129) | not_started | — |
| domain | `ROW-0130` | Inventory: Слоты, cursor stack, drag/quick-craft, shift-click, double-click, number keys, offhand, pick-block, throw, creative inventory, crafting/recipe book, component-aware stacks | `SRC-0130` (line 130) | not_started | — |
| domain | `ROW-0131` | Containers: Все vanilla screens закреплённой версии: chest/shulker/hopper и машины, торговля, обработка/ремонт/зачарование, карты/книги/signs, command/structure blocks при разрешённом режиме | `SRC-0131` (line 131) | not_started | — |
| domain | `ROW-0132` | World: Все измерения, chunk/light lifecycle, biome colors, weather/sky/celestial states, border, portals, world events, particles, block entities, maps, banners, paintings, moving blocks | `SRC-0132` (line 132) | not_started | — |
| domain | `ROW-0133` | Entities: Все видимые vanilla entity types, metadata, poses, equipment, passengers, interpolation/teleports, projectiles, status animations, player skins/capes и допустимые texture profiles | `SRC-0133` (line 133) | not_started | — |
| domain | `ROW-0134` | UI: HUD, hotbar, hearts/food/air/armor/effects, boss bars, scoreboard, team/tab list, titles, advancement/toast, chat, death/respawn, loading, pause, settings, accessibility | `SRC-0134` (line 134) | not_started | — |
| domain | `ROW-0135` | Text: Formatting/translatable components, hover/click actions с безопасностью, Unicode, fallback fonts, bidi/shaping, Cyrillic, CJK, IME, wrapping, narration/accessibility semantics | `SRC-0135` (line 135) | not_started | — |
| domain | `ROW-0136` | Audio: Music, positional/world/UI sounds, categories/volumes, weighted events, loops/streams, attenuation/pitch, subtitles, device change, mute/focus policy | `SRC-0136` (line 136) | not_started | — |
| domain | `ROW-0137` | Resource packs: Порядок наложения, namespaces, models/blockstates/item definitions, atlas/animation, fonts/languages/sounds, dynamic reload, pack filters/metadata version | `SRC-0137` (line 137) | not_started | — |
| domain | `ROW-0138` | Chat/services: Подписи/сертификаты и command signing где требуются версией; ошибки/неподписанные сообщения отображаются честно; reporting/social/Realms проходят отдельную проверку API | `SRC-0138` (line 138) | not_started | — |
| domain | `ROW-0139` | Local play: Создание/выбор/удаление/backup мира, lifecycle локального Solaris, LAN, paused/unpaused state, server settings, сохранение и восстановление | `SRC-0139` (line 139) | not_started | — |
| automation | `ROW-0672` | Discovery: describe, capabilities, list instances, health/readiness, limits, schemas, guide | `SRC-0672` (line 672) | not_started | — |
| automation | `ROW-0673` | Sessions: connect, connection status/wait Play, disconnect; target profile и server identity явны | `SRC-0673` (line 673) | not_started | — |
| automation | `ROW-0674` | Observation: player/pose/stats, inventory/cursor, container, target/raycast, loaded blocks/light, entities, recipe book, effects, chat/components | `SRC-0674` (line 674) | not_started | — |
| automation | `ROW-0675` | Waits: state change, exact block, chunk-loaded, entity spawned/moved/removed, inventory condition, health, screen/view state, audio event, frame completion | `SRC-0675` (line 675) | not_started | — |
| automation | `ROW-0676` | Input: look/look-at, bounded semantic keys/mouse edges, hotbar, offhand, sprint/sneak, inventory open/close, release-all | `SRC-0676` (line 676) | not_started | — |
| automation | `ROW-0677` | Interaction: break/place/use/use-on/interact entity, attack once/hold where legitimate, drop, pick-block in permitted game mode | `SRC-0677` (line 677) | not_started | — |
| automation | `ROW-0678` | Containers: click/quick-move/drag/number-key/button, expected screen/container revision, server-observed result | `SRC-0678` (line 678) | not_started | — |
| automation | `ROW-0679` | UI: inspect semantic tree, focus/type/select/scroll/click, inspect deny reason, wait view, Loader action/request view | `SRC-0679` (line 679) | not_started | — |
| automation | `ROW-0680` | Navigation: bounded movement in known world using normal inputs; arrival predicate, progress/stuck reasons/cancel | `SRC-0680` (line 680) | not_started | — |
| automation | `ROW-0681` | Solaris: activated bundles/permissions, namespaced view/markers/selection, action receipt; only data sent to this session | `SRC-0681` (line 681) | not_started | — |
| automation | `ROW-0682` | Rendering: capture screenshot, inspect frame metadata/passes/draw stats, wait mesh/resource ready, optional bounded debug overlays | `SRC-0682` (line 682) | not_started | — |
| automation | `ROW-0683` | Audio: inspect playback state, start bounded capture from configured sink, read levels/artifact; distinguish command vs output | `SRC-0683` (line 683) | not_started | — |
| automation | `ROW-0684` | Performance: start/stop named capture, metrics snapshot, marker/timeline, memory residency, queues, shader/mesh compilation stats | `SRC-0684` (line 684) | not_started | — |
| automation | `ROW-0685` | Replay: start/stop bounded recording, load allowed replay, seek in local replay, export/redact; no arbitrary runtime file access | `SRC-0685` (line 685) | not_started | — |
| automation | `ROW-0686` | Scenario: list/run/status/cancel predefined or schema-validated local scenarios, assertions, evidence bundle | `SRC-0686` (line 686) | not_started | — |
| automation | `ROW-0687` | Controller: acquire/renew/release input lease, cancel action, emergency release; status доступен человеку | `SRC-0687` (line 687) | not_started | — |
| performance | `P01` | P01 Traversal / 1080p, Parity, view 16, normal particles, разнообразный terrain, обычные mobs, прогретый маршрут / p50 ≤8.3 ms; p95 ≤12.5 ms; p99 ≤16.7 ms | `SRC-0783` (line 783) | not_started | — |
| performance | `P02` | P02 Streaming / Тот же profile, новая местность/быстрое движение и teleport, отдельно cold caches / p95 ≤16.7 ms; p99 ≤25 ms; очередь bounded, stall >50 ms отдельно учитывается | `SRC-0784` (line 784) | not_started | — |
| performance | `P03` | P03 Battle / 1024 реально присутствующих бойца, 50 player avatars, terrain/оборудование/снаряды и звук, видимость зафиксирована / p95 ≤16.7 ms; p99 ≤25 ms | `SRC-0785` (line 785) | not_started | — |
| performance | `P04` | P04 Crowd stress / 4096 бойцов с репрезентативной экипировкой, та же камера/геометрия; точный visible-set / p95 ≤33.3 ms; p99 ≤50 ms; не удалять видимых участников | `SRC-0786` (line 786) | not_started | — |
| performance | `P05` | P05 Client-state scale / 100k полученных entity records, фиксированный subset 4096 visible-eligible, burst metadata / Bounded memory/queues; отсутствие O(100k) полного render rebuild/frame; это не 100k одновременно отрисованных тел | `SRC-0787` (line 787) | not_started | — |
| performance | `P06` | P06 Memory / P01 steady 30 min / Исходная цель process resident RAM ≤2 GiB и tracked GPU allocations ≤2.5 GiB; driver overhead отдельно | `SRC-0788` (line 788) | not_started | — |
| performance | `P07` | P07 Reload/soak / 20 join/disconnect + reload cycles, затем 2 h mixed play / Нет монотонного роста live resources; данные и hooks предыдущего сервера не остаются активными | `SRC-0789` (line 789) | not_started | — |
| performance | `P08` | P08 MCP idle / Тот же scene, endpoint on/off без запросов / overhead стремится к нулю; gate ≤1% median CPU frame и отсутствие периодических world scans | `SRC-0790` (line 790) | not_started | — |
| performance | `P09` | P09 MCP active / 10 bounded observations/s + 1 causal action/s, без heavy capture / p95 regression ≤5% на том же baseline; корректная cancellation и bounded queue | `SRC-0791` (line 791) | not_started | — |
| performance | `P10` | P10 Interaction / Близкий одиночный block edit и GUI action при рабочем FPS / Source-apply → first correct submitted frame p95 ≤50 ms; server/network latency измеряется отдельно | `SRC-0792` (line 792) | not_started | — |
| acceptance | `AUTH-01` | Собственный зарегистрированный клиент выполняет разрешённый sign-in, проверяет доступ и подключается к online-mode vanilla; credentials не попадают в MCP/log | `SRC-0871` (line 871) | not_started | — |
| acceptance | `AUTH-02` | Expired/revoked token, отсутствие доступа, user cancel и network error различаются; нет fallback на чужую identity | `SRC-0872` (line 872) | not_started | — |
| acceptance | `ASSET-01` | Clean profile получает нужные source assets, hashes/size совпадают, повторный запуск использует verified cache | `SRC-0873` (line 873) | not_started | — |
| acceptance | `ASSET-02` | Truncated/corrupt download и concurrent install не публикуют partially-valid cache | `SRC-0874` (line 874) | not_started | — |
| acceptance | `ASSET-03` | ZIP slip, duplicate paths, decompression bomb, oversized PNG/font/OGG и циклическая модель отклоняются bounded | `SRC-0875` (line 875) | not_started | — |
| acceptance | `ASSET-04` | Release archive и source tree не содержат запрещённых Mojang bytes; provenance/license manifest проходит review | `SRC-0876` (line 876) | not_started | — |
| acceptance | `NET-01` | Покрыты все состояния/переходы и обязательные packets закреплённой версии; не только Login+Play | `SRC-0877` (line 877) | not_started | — |
| acceptance | `NET-02` | Encryption/compression boundaries, malformed lengths/VarInts/NBT и limits покрыты fuzz/property tests | `SRC-0878` (line 878) | not_started | — |
| acceptance | `NET-03` | Reconfiguration, transfer/respawn/dimension change не допускают stale packet/job commit | `SRC-0879` (line 879) | not_started | — |
| acceptance | `NET-04` | Packet burst не вызывает unbounded memory, semantic reorder или silent drop необходимых событий | `SRC-0880` (line 880) | not_started | — |
| acceptance | `MOVE-01` | Набор stairs/slabs/doors/fences/fluids/ladder/honey/ice/edge-crouch проходит differential movement tests | `SRC-0881` (line 881) | not_started | — |
| acceptance | `MOVE-02` | Sprint/swim/crawl/glide/vehicle/mount/flight и server corrections дают version-correct поведение | `SRC-0882` (line 882) | not_started | — |
| acceptance | `MOVE-03` | Низкий/меняющийся FPS не ускоряет movement, break progress или input cadence | `SRC-0883` (line 883) | not_started | — |
| acceptance | `MOVE-04` | Большие координаты и origin shift не дают jitter/ложный raycast/network drift | `SRC-0884` (line 884) | not_started | — |
| acceptance | `INT-01` | Breaking/place/use/attack/main/offhand проходят ordinary input с server-observed результатами | `SRC-0885` (line 885) | not_started | — |
| acceptance | `INT-02` | Bow/crossbow/shield/fishing/food/bucket и отмена использования сохраняют корректные lifecycle/events | `SRC-0886` (line 886) | not_started | — |
| acceptance | `INV-01` | Все click modes, cursor stack, crafting и recipe book совпадают с oracle | `SRC-0887` (line 887) | not_started | — |
| acceptance | `INV-02` | Закрытие/повторное открытие контейнера и stale state ID не дают dups/ghost success | `SRC-0888` (line 888) | not_started | — |
| acceptance | `INV-03` | Все container screen families из compatibility matrix проходят реальные UI сценарии | `SRC-0889` (line 889) | not_started | — |
| acceptance | `ENT-01` | Каждый vanilla entity/block-entity family имеет проверенные geometry/metadata/pose/layers без обязательных placeholders | `SRC-0890` (line 890) | not_started | — |
| acceptance | `ENT-02` | Despawn+reuse numeric ID и dimension transition не переиспользуют stale animation/action handle | `SRC-0891` (line 891) | not_started | — |
| acceptance | `ENT-03` | Hidden-by-culling entity продолжает принимать нужный state; при появлении не теряет damage/pose/equipment | `SRC-0892` (line 892) | not_started | — |
| acceptance | `VIS-01` | Все block model families, rotations, multipart, tint, animations и item-model graphs отображены | `SRC-0893` (line 893) | not_started | — |
| acceptance | `VIS-02` | Greedy meshing не меняет UV, AO/light gradients, seams/diagonal и partial block visibility | `SRC-0894` (line 894) | not_started | — |
| acceptance | `VIS-03` | Glass/water/cutout/particles/entities в разных слоях проходят transparent regression scenes | `SRC-0895` (line 895) | not_started | — |
| acceptance | `VIS-04` | Camera teleport/fast turn/broken occluder/new entity не дают false-negative visibility | `SRC-0896` (line 896) | not_started | — |
| acceptance | `VIS-05` | Reload не показывает new mesh с old atlas; stale jobs и GPU handles отвергнуты | `SRC-0897` (line 897) | not_started | — |
| acceptance | `VIS-06` | Near edit-to-visible измерен под streaming load; patch/full mesh не дублируют faces | `SRC-0898` (line 898) | not_started | — |
| acceptance | `VIS-07` | Sky/weather/portal/underwater/hand/outline/glow и все HUD layers проходят scene checks | `SRC-0899` (line 899) | not_started | — |
| acceptance | `VIS-08` | Portable и GPU-driven paths дают согласованный результат на одинаковом visual profile | `SRC-0900` (line 900) | not_started | — |
| acceptance | `UI-01` | Все базовые menus/HUD/screens работают mouse+keyboard и имеют актуальное semantic tree | `SRC-0901` (line 901) | not_started | — |
| acceptance | `UI-02` | Cyrillic/CJK/bidi/IME, DPI scaling, resizing/fullscreen/focus и clipboard policy проверены на ОС | `SRC-0902` (line 902) | not_started | — |
| acceptance | `UI-03` | Disabled/hidden/stale UI actions не обходятся через MCP | `SRC-0903` (line 903) | not_started | — |
| acceptance | `AUDIO-01` | Positional/loop/music/category/mute/subtitles и disconnect cleanup подтверждены output capture | `SRC-0904` (line 904) | not_started | — |
| acceptance | `AUDIO-02` | Device switch/потеря устройства не блокируют client loop и не оставляют вечные voices | `SRC-0905` (line 905) | not_started | — |
| acceptance | `SOL-01` | Native platform объявлена честно; проверены manifest/ACK compatibility и отказ неподдерживаемых features | `SRC-0906` (line 906) | not_started | — |
| acceptance | `SOL-02` | Отказ permissions не загружает/активирует запрещённый bundle; changed scopes требуют нового решения | `SRC-0907` (line 907) | not_started | — |
| acceptance | `SOL-03` | Namespaced views/HUD/input/world selection/previews/items/blocks/sounds работают с реальными fixtures | `SRC-0908` (line 908) | not_started | — |
| acceptance | `SOL-04` | Смена сервера удаляет все owner-specific UI/bindings/sounds/handles без утечки между соединениями | `SRC-0909` (line 909) | not_started | — |
| acceptance | `WASM-01` | Server WASM не исполняется на клиенте; client-WASM имеет собственный проверенный WIT contract | `SRC-0910` (line 910) | not_started | — |
| acceptance | `WASM-02` | Infinite loop, allocation flood, host-call flood, trap, malformed module и большой copy bounded | `SRC-0911` (line 911) | not_started | — |
| acceptance | `WASM-03` | Guest не читает tokens/files/hidden state и не имеет raw GPU/MCP authority | `SRC-0912` (line 912) | not_started | — |
| acceptance | `WASM-04` | Reload/disable/crash не повторяют серверный action и не повреждают соседний addon | `SRC-0913` (line 913) | not_started | — |
| acceptance | `MCP-01` | Старый catalog/semantic contract проходит shared fixture tests, новые tools обнаруживаются через schemas | `SRC-0914` (line 914) | not_started | — |
| acceptance | `MCP-02` | stdio и HTTP проходят соответствующие protocol-version conformance tests без смешения lifecycle | `SRC-0915` (line 915) | not_started | — |
| acceptance | `MCP-03` | Unauthorized/origin-rebinding/body-limit/foreign-instance requests отвергаются | `SRC-0916` (line 916) | not_started | — |
| acceptance | `MCP-04` | Readonly observe работает одновременно с hold input; два controllers не получают одну write lease | `SRC-0917` (line 917) | not_started | — |
| acceptance | `MCP-05` | Cancel/timeout/focus-loss/disconnect/expiry release все keys и прекращают navigation | `SRC-0918` (line 918) | not_started | — |
| acceptance | `MCP-06` | Wait не пропускает событие между read/subscribe и не busy-polls idle world | `SRC-0919` (line 919) | not_started | — |
| acceptance | `MCP-07` | Один action_id не выполняется дважды; timeout после dispatch не возвращает fabricated success | `SRC-0920` (line 920) | not_started | — |
| acceptance | `MCP-08` | Snapshot consistency, epochs, pagination, unknown/unloaded и truncation отражены точно | `SRC-0921` (line 921) | not_started | — |
| acceptance | `MCP-09` | Agent проходит survival crafting и container scenario без privileged mutation hooks | `SRC-0922` (line 922) | not_started | — |
| acceptance | `MCP-10` | Screenshot подтверждает actual framebuffer; offscreen/state-only flags не выданы за onscreen evidence | `SRC-0923` (line 923) | not_started | — |
| acceptance | `MCP-11` | Startup descriptor/guide/host config позволяют новому агенту обнаружить exact instance без знания прошлой беседы | `SRC-0924` (line 924) | not_started | — |
| acceptance | `MCP-12` | Недоверенный текст в чате/книге не меняет permissions, tool catalog или actor instructions | `SRC-0925` (line 925) | not_started | — |
| acceptance | `SP-01` | Локальный Solaris start/save/exit/crash/restart и LAN выполняются без JVM клиента | `SRC-0926` (line 926) | not_started | — |
| acceptance | `SP-02` | World import сохраняет оригинал и явно отвергает неподдержанное вместо data loss | `SRC-0927` (line 927) | not_started | — |
| acceptance | `SP-03` | `Singleplayer-vanilla-parity` не объявляется без отдельного server/worldgen coverage | `SRC-0928` (line 928) | not_started | — |
| acceptance | `PERF-01` | P01–P10 имеют raw measurements, equality settings и неизменённые workloads | `SRC-0929` (line 929) | not_started | — |
| acceptance | `PERF-02` | Backend feature unavailable, GPU loss и memory pressure дают корректную degraded/error policy | `SRC-0930` (line 930) | not_started | — |
| acceptance | `PERF-03` | Многочасовой mixed run не накапливает handles, watchers, textures, wasm stores и input leases | `SRC-0931` (line 931) | not_started | — |
| acceptance | `REL-01` | Чистая сборка/установка на целевых ОС, правильные licenses/signatures и pinned dependencies | `SRC-0932` (line 932) | not_started | — |
| acceptance | `REL-02` | Каждый unsupported service/pack-extension/version виден в compatibility matrix; нет полного claim с пустыми обязательными строками | `SRC-0933` (line 933) | not_started | — |
| platform | `PLAT-LINUX` | Linux x86-64 / Vulkan; mandatory first desktop | `SRC-0051` (line 51) | not_started | — |
| platform | `PLAT-WINDOWS` | Windows x86-64 / DX12; mandatory first desktop | `SRC-0051` (line 51) | not_started | — |
| platform | `PLAT-MACOS` | macOS arm64 / Metal; mandatory before full desktop release | `SRC-0051` (line 51) | not_started | — |
| hardware | `GPU-AMD-INTEL` | AMD/Intel capability baseline | `SRC-0775` (line 775) | not_started | — |
| service | `MICROSOFT-XBOX-MINECRAFT` | Own application registration, OAuth, entitlement/profile/session join; external access gate | `SRC-0084` (line 84) | not_started | — |
| service | `REALMS-SOCIAL-REPORTING` | Official API/access evidence; absence blocks service completeness | `SRC-0765` (line 765) | not_started | — |
| server | `VANILLA-TARGET` | Official target vanilla server | `SRC-0763` (line 763) | not_started | — |
| server | `SOLARIS-BASE` | Current Solaris without addons | `SRC-0763` (line 763) | not_started | — |
| server | `SOLARIS-BUNDLES` | Solaris standard presentation bundles | `SRC-0763` (line 763) | not_started | — |
| server | `SOLARIS-WASM` | Solaris client-WASM scenarios | `SRC-0763` (line 763) | not_started | — |
| server | `ADVERSARIAL` | Adversarial server fixture | `SRC-0763` (line 763) | not_started | — |
| interop | `PROXY-PAPER` | Separately declared interoperability only after verification | `SRC-0763` (line 763) | not_started | — |
| pack-extension | `SHADER-OVERRIDES` | Version-specific vanilla shader overrides audit; not implied by PNG/JSON support | `SRC-0143` (line 143) | not_started | — |
| pack-extension | `GLSL-OPTIFINE-IRIS` | No automatic shader compatibility; separate adapter/trust contract | `SRC-1055` (line 1055) | not_started | — |
| out_of_scope | `BEDROCK` | Bedrock protocol outside first target | `SRC-0049` (line 49) | not_started | — |
| out_of_scope | `JAVA-MODS` | Fabric/Forge/NeoForge JAR execution, JVM and arbitrary Java mixins outside target | `SRC-0049` (line 49) | not_started | — |
| out_of_scope | `UNOFFICIAL-SHADERPACKS` | All unofficial shaderpack-extension compatibility not promised | `SRC-0049` (line 49) | not_started | — |
| out_of_scope | `NON-DESKTOP` | Web, Android, iOS and consoles require separate request | `SRC-0051` (line 51) | not_started | — |
| stage | `C00` | контракт, provenance и измерительный стенд | `SRC-0945` (line 945) | accepted | `.analysis/evidence/C00/acceptance-receipt.json`, `independent-review.json`; explicit own-auth/native-Loader/release-licensing blockers |
| stage | `C01` | реальное окно, render loop и MCP до игровой логики | `SRC-0951` (line 951) | accepted | `.analysis/evidence/C01/acceptance-receipt.json`, `independent-review.json`; actual Linux/X11 Vulkan GPU window, resize/focus/Escape, authenticated HTTP+same-instance stdio, saturation-safe cleanup |
| stage | `C02` | настоящий vanilla session и первый gameplay loop | `SRC-0957` (line 957) | accepted | `.analysis/evidence/C02/acceptance-receipt.json`, `independent-review.json`; both actual official/Solaris ordinary SURVIVAL loops, authoritative inventory, native/GPU frames and authorized isolated PCM; retained remesh/light/terminal-queue failures plus corrected regressions and final-binary native/EOF smoke; no hardware-audibility, complete gameplay re-run on final binary or full-client claim |
| stage | `C03` | масштаб terrain без потери картинки | `SRC-0963` (line 963) | not_started | — |
| stage | `C04` | entities, все взаимодействия, containers и UI | `SRC-0969` (line 969) | not_started | — |
| stage | `C05` | Solaris native compatibility | `SRC-0975` (line 975) | not_started | — |
| stage | `C06` | полноценный client-WASM host | `SRC-0981` (line 981) | not_started | — |
| stage | `C07` | GPU-driven fast path и большой бой | `SRC-0987` (line 987) | not_started | — |
| stage | `C08` | singleplayer, services и desktop completeness | `SRC-0993` (line 993) | not_started | — |
| stage | `C09` | agents, reliability, security, distribution | `SRC-0999` (line 999) | not_started | — |
| stage | `C10` | конечная приёмка и дальнейшие эксперименты | `SRC-1005` (line 1005) | not_started | — |
