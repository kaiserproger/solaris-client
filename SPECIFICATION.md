# Solaris Native Client specification

## Authority and immutable requirement IDs

Authority: `/home/kaiserroman/Downloads/SOLARIS_NATIVE_CLIENT_SPEC_FOR_OPUS.md`, edition 2026-10-01, SHA-256 `645ca42ff3f3083b561acc84a707ea5d4a27fcd383da1cfdc57ab6276f008c2a`, 1116 lines. The complete original text is retained below without omissions, including proposed contracts, tables, code examples, constraints, sources, and non-acceptance caveats. Russian wording is authoritative; introductions in English do not narrow it.

`SRC-NNNN` permanently identifies the original source line NNNN. Every nonblank source line has an ID in the ledger, so every paragraph, list clause, table row, diagram and example is traceable. Adjacent IDs form a paragraph/block requirement; headings provide context, not a substitute for clauses. Acceptance names (AUTH-01 etc.), performance gates (P01–P10), and stages (C00–C10) are secondary semantic IDs, not replacements for SRC IDs. Do not renumber SRC IDs when implementation changes. Future requirements get new IDs with explicit provenance and approved changes; retain this baseline.

Implementation status and evidence live in [COMPATIBILITY_MATRIX.md](COMPATIBILITY_MATRIX.md); the sole work cursor lives in [LONG_TERM_PLAN.md](LONG_TERM_PLAN.md). No clause in this baseline asserts that an implementation or performance gate has passed.

## Complete authoritative baseline

# Solaris Native Client — техническое задание для Opus

**Редакция:** 1 октября 2026 года.  
**Статус:** проект целевой реализации; это не отчёт о существующем Rust-клиенте или измеренной производительности.  
**Рабочее имя:** `solaris-client`. Независимый репозиторий рядом с `solaris`, `solaris-loader`, `solaris-default-plugins`.  
**Назначение:** самостоятельный open-source клиент Minecraft Java Edition на Rust, собственный оптимизированный рендерер, нативная поддержка Solaris, безопасные клиентские WASM-расширения и полноценная агентская автоматизация через MCP.

## Инструкция исполняющему агенту

Прочитай этот документ целиком, затем сверяй утверждения о существующем API с актуальным кодом трёх репозиториев. Источники исследования и конкретные пути приведены в конце. Локальные рабочие деревья содержали незакоммиченные изменения; их нельзя сбрасывать, заменять содержимым GitHub или считать частью нового клиента.

Сначала создай в **новом клиентском репозитории** `SPECIFICATION.md`, `LONG_TERM_PLAN.md`, `COMPATIBILITY_MATRIX.md`, `docs/AGENT_TESTING.md` и короткий `AGENTS.md`, опираясь на это ТЗ. Назначь один действующий курсор работы. Существующий серверный план не заменять. Реализацию вести вертикальными этапами из §24; не объявлять результат полным клиентом после появления окна, чанков и ходьбы.

Номера версий будущих client-WASM API, CLI и новых инструментов ниже — **предлагаемые контракты**, а не утверждение, что они уже существуют. Никаких фиктивных проверок, выдуманных FPS, молчаливого удаления требований, подмены отсутствующих механик заглушками и расширения разрешений без согласованного контракта. Любая завершённая возможность должна иметь воспроизводимое доказательство.

---

## 1. Продукт и границы обещания

### 1.1. Цель

Пользователь запускает нативный клиент, авторизуется своей учётной записью, получает необходимые оригинальные ресурсы из разрешённых источников и играет на совместимом vanilla-сервере либо Solaris. Основные движения, взаимодействия, инвентарь, звук, визуальные состояния, интерфейсы и сетевое поведение воспроизводят закреплённую Java Edition. На Solaris автоматически согласуются дополнительные данные, интерфейсы и возможности аддонов. Агент может выполнять реальные игровые сценарии, видеть подтверждённые изменения и собирать доказательства рендеринга без GUI-скриптинга по координатам пикселей.

Клиентская реализация, шейдеры и инструменты проекта открыты. Minecraft-ассеты и принадлежащий Mojang код от этого не становятся open-source; они не входят в исходники и бинарные релизы проекта.

### 1.2. Первая закреплённая версия

Первый compatibility target — **Minecraft Java Edition 26.1.2**, поскольку именно эта версия фигурирует в исследованном Solaris Loader. Это осознанная совместимость с проектом, не заявление о новейшей Minecraft. Официальная страница подтверждает существование релиза [S01].

На этапе C00 создать `compatibility.lock`: game version, протокол, hashes исходных метаданных/ресурсов, используемые ревизии Solaris contracts, resource-pack/data formats, Rust toolchain, версии зависимостей. Числовой protocol ID и структуры пакетов получить из проверенных version-specific источников и тестов; не переносить числа из памяти или соседней версии.

Не начинать с поддержки десяти версий одновременно. Расширение матрицы не должно превращать игровой код в россыпь version checks: различия сосредоточены в protocol/data adapters. Первая полная версия остаётся обязательной до масштабирования числа версий.

### 1.3. Полнота разделяется на проверяемые уровни

| Уровень | Что нужно доказать |
|---|---|
| `Java-client-compatible` | Все обязательные клиентские функции закреплённой версии, а не только несколько серверных сценариев |
| `Solaris-native` | Совместимые Loader-контракты, permissions, bundles, интерфейсы, предметы/блоки/звук; новые возможности только через negotiated extension |
| `Client-WASM` | Самостоятельный ограниченный клиентский ABI и полноценный lifecycle безопасных расширений |
| `Singleplayer-Solaris` | Локальный Rust-сервер Solaris, управление миром, сохранение, пауза/LAN и явно выбранные правила |
| `Singleplayer-vanilla-parity` | Дополнительно подтверждена полнота серверной симуляции и worldgen; успех клиента этого не доказывает |
| `Official-services` | Реально проверены доступные официальные службы, включая нужные права приложения; неподдержанное явно указано |

Один ярлык `full Minecraft` нельзя использовать, если обязательные строки матрицы ещё не реализованы. При этом этапность не вычёркивает полный конечный объём.

### 1.4. Вне первой целевой поставки

Bedrock protocol, исполнение Fabric/Forge/NeoForge JAR-модов, эмуляция JVM, произвольных Java mixins и совместимость со всеми неофициальными shaderpack-расширениями не являются частью Java-client parity. Их отсутствие должно быть явно отражено. Полная собственная клиентская реализация не обещает бинарную совместимость с Java-модами.

Linux x86-64 и Windows x86-64 — обязательные первые desktop-платформы. macOS arm64 — обязательная последующая платформа до объявления полного desktop-релиза. Web, Android, iOS и консоли не включать в этот план без отдельного запроса.

## 2. Основные решения и отклонённые подходы

| Решение | Обоснование и граница |
|---|---|
| Rust + `winit` + собственный renderer поверх `wgpu` | Контролируемые данные/кадр и portable GPU backend. Не писать заново Vulkan/Metal/DX12 до обнаружения конкретного ограничителя |
| Небольшое явное client-core вместо полного игрового движка | Мир и protocol lifecycle специфичны; не покупать удобство general-purpose engine ценой двух моделей состояния |
| Авторитетные packet-state и локальная prediction раздельны | Предсказанное разрушение/перенос предмета не считается подтверждённым серверным результатом |
| Главный поток обслуживает окно; session owner обслуживает игру; render owner владеет GPU | Долгая распаковка, remeshing, WASM и сеть не блокируют ввод/кадр |
| Оптимизация данных и количества работы раньше SIMD | Rust сам по себе не устраняет лишние обходы, draw calls, allocations и синхронизации |
| GPU-driven drawing как приоритетный fast path | Не требует одного CPU draw на сущность; baseline остаётся на той же модели данных |
| MCP использует обычный input/action path | Тест проверяет настоящий клиент; отдельного «бот-клиента с похожими функциями» нет |
| Client-WASM отдельно от server `solaris:plugin` | Серверная экономика/инвентарь/боевые решения не исполняются повторно на машине игрока |
| Исходные ассеты отдельно от производных caches | Проверяемое происхождение, отсутствие копий Mojang в релизе, быстрый повторный запуск |
| Полнота и быстродействие принимаются независимыми gates | Быстрые кубы не заменяют воду, прозрачность, мобов, шрифты, инвентарь и звук |

Не начинать с ECS-фреймворка ради самого ECS, task scheduler framework, universal engine abstraction, собственного DSL для всего или distributed desktop-runtime. Выделять crate по границе ответственности/переиспользования, а не для каждого небольшого файла. Любая `unsafe`-оптимизация требует локального инварианта, тестов и измеренного эффекта.

## 3. Законность поставки, происхождение ресурсов, авторизация

### 3.1. Ограничения поставки

EULA запрещает распространять созданное Mojang ПО/контент без разрешения и требует не выдавать сторонние инструменты за официальные [S02, S03]. Скачивание ресурсов с официального CDN само по себе **не является юридическим разрешением на любую самостоятельную реализацию**. До публичного выпуска нужен отдельный licensing review конкретного способа совместимости и поставки; этот документ не заменяет юридическое заключение.

Релиз содержит собственные бинарники и только ресурсы с подходящей лицензией. Не включать client/server JAR, извлечённые текстуры, модели, звуки, fonts, скопированные Mojang shaders, декомпилированные Java sources, собранные из них производные binary caches. Исключить их из Git, release archives, container images и общедоступного CI cache.

Нельзя автоматически считать код оптимизационных модов пригодным для копирования. На дату исследования Sodium в изученном upstream указывает **Polyform Shield 1.0.0**, Nvidium — LGPL-3.0 [S08, S09]. Изучение подходов и самостоятельная реализация отделяются от переноса кода; каждый заимствованный файл/алгоритмическая реализация проходит provenance/license review. Для собственного кода предпочтительна согласованная с Solaris MIT/Apache-2.0 поставка без случайно несовместимых включений.

### 3.2. Авторизация

В пользовательском режиме поддержать официальный Microsoft sign-in с собственным корректно зарегистрированным public-client приложением. Предпочтение — системный браузер и authorization code + PKCE; device flow только там, где он допустим для приложения и сценария. Не принимать пароль в собственном UI и не использовать password grant [S04].

Далее реализуется поддерживаемая цепочка Xbox/Minecraft authentication, получение профиля, проверка доступа к игре и session join. Точный набор endpoints/scopes/registration requirements проверяется на C00. Успех Microsoft OAuth не доказывает доступ к Minecraft Services. Получение нужных разрешений для собственного client ID — ранний внешний gate; не заменять его чужим ID, заимствованным секретом или обходом проверки.

Access/refresh tokens хранить через OS credential store. В отладочном логе, replay, MCP, crash report и command line их быть не должно. В случае отсутствия безопасного хранилища — работа без долговременного сохранения либо явное локальное решение, не скрытый plaintext. Session credentials передавать дочерним процессам только через ограниченный IPC, не в URL.

Offline/test identity допускается только в явно выбранном локальном/разрешённом тестовом профиле. Такой прогон не закрывает online authentication. Реальный offline singleplayer использует ранее законно полученные ресурсы и зафиксированные правила доступа; отсутствие сети не обосновывает подмену чужого аккаунта.

### 3.3. Загрузчик ассетов

Поток:

```text
официальный version manifest
  → выбранный version descriptor
  → client archive + asset index
  → нужные asset objects
  → verified source cache
  → собственная компиляция моделей/текстур/шрифтов
  → versioned derived cache
```

Внешний asset index не считать полным перечнем всех клиентских данных: нужны также ресурсы из client archive; формы и поведение, определённые программным кодом, реализуются самостоятельно. На C00 составить inventory фактических `assets/` и index закреплённой версии. Не предполагать, что все модели мобов и все анимации уже даны JSON-файлами.

Bootstrap URL и разрешённые официальные origins фиксировать в конфигурации дистрибутива. Скачать descriptor по ссылке из доверенного manifest, проверить предоставленные digest и размер. Для оригинального формата сохранить его проверки SHA-1, если именно они заданы; для своего cache дополнительно вычислять SHA-256. SHA-256 собственного файла не подменяет проверку происхождения.

Загрузка ограничена по parallelism, bytes, timeouts и повторным попыткам; resumable только при подтверждённой идентичности объекта. `.partial → verify → atomic publish`; два клиента используют lock/дедупликацию. Не запускать Java или исполняемые файлы из загружаемого archive. Безопасно обрабатывать ZIP paths, symlinks, duplicate entries, размер после распаковки, compression ratio, depth JSON/NBT, dimensions изображений и duration аудио.

Не доверять произвольному download URL от игрового сервера: resource packs и Solaris bundles проходят отдельную policy, consent, лимиты и origin isolation. Redirect повторно проверяется. Закрыты `file:`, произвольная локальная сеть/metadata endpoints и извлечение credentials из иных origins. Разрешённые localhost test URLs оформляются тестовым профилем.

Существующий ресурсный cache игрока можно импортировать read-only после проверки версии/digest; не модифицировать его обычную `.minecraft`. При недоступном CDN использовать уже проверенный cache либо показать точную ошибку. Заменять Mojang зеркалом без согласия и скрыто игнорировать checksum запрещено.

### 3.4. Воспроизводимая независимая разработка

Общие unit/renderer tests используют собственный redistributable fixture pack. Локальные parity-тесты могут использовать законно полученные оригинальные resources. Их machine-readable receipts содержат hashes/происхождение без упаковки ассетов в публичный артефакт.

Получение наблюдаемого поведения через vanilla oracle и перенос copyrighted реализации — разные операции. Вводить журнал происхождения для сложных алгоритмов и таблиц. Не заявлять clean-room только потому, что Java переписана на Rust.

## 4. Полная карта клиентской функциональности

Для каждой строки матрицы вести статус `not_started / implemented / oracle_verified / integrated / accepted`, тестовые IDs и поддерживаемые варианты. `unsupported` допустим как честный статус разработки, но не как закрытие обязательного требования.

| Область | Обязательные функции конечного target |
|---|---|
| Launcher/shell | Профили, account lifecycle, список серверов, адреса IPv4/IPv6/DNS, необходимые SRV-правила, direct connect, progress, cancel/retry, понятные disconnects |
| Connection | Status, login, online session, encryption/compression, configuration, registries/tags, resource packs, Play, reconfiguration, transfers/cookies там, где определены выбранной версией |
| Player | Ходьба, sprint/sneak/jump, crouch/swim/crawl, вода/лава, лестницы, лед/slime/honey/soul blocks, elytra, vehicles/mounts, flight/creative/spectator, knockback, collisions, camera |
| Interaction | Raycast, breaking stages, placement, use/use-on/use-entity, main/off-hand, entity attacks, cooldowns, bows/crossbows/fishing, food, buckets, shields, reach и block/entity shapes |
| Inventory | Слоты, cursor stack, drag/quick-craft, shift-click, double-click, number keys, offhand, pick-block, throw, creative inventory, crafting/recipe book, component-aware stacks |
| Containers | Все vanilla screens закреплённой версии: chest/shulker/hopper и машины, торговля, обработка/ремонт/зачарование, карты/книги/signs, command/structure blocks при разрешённом режиме |
| World | Все измерения, chunk/light lifecycle, biome colors, weather/sky/celestial states, border, portals, world events, particles, block entities, maps, banners, paintings, moving blocks |
| Entities | Все видимые vanilla entity types, metadata, poses, equipment, passengers, interpolation/teleports, projectiles, status animations, player skins/capes и допустимые texture profiles |
| UI | HUD, hotbar, hearts/food/air/armor/effects, boss bars, scoreboard, team/tab list, titles, advancement/toast, chat, death/respawn, loading, pause, settings, accessibility |
| Text | Formatting/translatable components, hover/click actions с безопасностью, Unicode, fallback fonts, bidi/shaping, Cyrillic, CJK, IME, wrapping, narration/accessibility semantics |
| Audio | Music, positional/world/UI sounds, categories/volumes, weighted events, loops/streams, attenuation/pitch, subtitles, device change, mute/focus policy |
| Resource packs | Порядок наложения, namespaces, models/blockstates/item definitions, atlas/animation, fonts/languages/sounds, dynamic reload, pack filters/metadata version |
| Chat/services | Подписи/сертификаты и command signing где требуются версией; ошибки/неподписанные сообщения отображаются честно; reporting/social/Realms проходят отдельную проверку API |
| Local play | Создание/выбор/удаление/backup мира, lifecycle локального Solaris, LAN, paused/unpaused state, server settings, сохранение и восстановление |

Особенно не забыть функциональность, которую обычно теряют voxel-прототипы: точные waterlogged/slab/stair shapes, entity/block-entity animation, составные item components, состояние cursor stack, text layout, portals, beds, server corrections, звук и безопасный resource reload.

Для нестандартных shader overrides в vanilla resource packs провести отдельную ревизию формата выбранной версии. Поддержку нельзя обещать просто из наличия PNG/JSON decoder. Если ресурсный пакет меняет исполняемые shaders, нужен проверяемый адаптер ограниченного shader dialect/контракта либо явно неподдержанный вариант до реализации. Произвольный GLSL/OptiFine/Iris pack не становится автоматически WGSL-программой.

## 5. Архитектура и владельцы данных

### 5.1. Начальная структура

Предлагается 6 производственных crates, test utilities внутри workspace:

```text
client-app         window, lifecycle, settings, process startup
client-core        packet-applied world, prediction, actions, UI semantics
client-render      scene extraction consumer, resources, meshes, GPU, capture
client-content     assets, resource-pack stack, baking, codecs, derived cache
client-solaris     Loader adapter + client-WASM host contracts
client-automation  typed control API, MCP adapters, scenarios, evidence
```

Сетевой codec, auth, audio и platform modules сначала могут быть модулями этих crates; отделять после появления реального независимого consumer. Общие `mc-protocol`, `mc-nbt`, `mc-domain` брать из version-pinned Solaris, если dependency audit подтверждает пригодность. Изученный `mc-protocol/Cargo.toml` зависит от `mc-data`, `mc-domain`, `mc-nbt`, а не от `mc-net` [R06]. Это кандидат на reuse, **не доказательство**, что там уже есть все clientbound decoders и online auth.

Не тащить `mc-net`, весь `mc-server`, `SessionRegistry` и persistence server-world в multiplayer client. Сначала audit public APIs/feature boundaries, затем одна небольшая upstream extraction там, где она действительно нужна. Не копировать protocol enums независимо в двух репозиториях и не проводить большой server refactor как условие первого окна.

### 5.2. Время и потоки

```text
OS event loop ── input edges ──→ session owner
network IO ── ordered decoded packets ──→ session owner
MCP/WASM ── typed bounded intents ──→ session owner

session owner:
  apply packets → local prediction → actions → compact change stream
                                            ↓
content/mesh workers ← immutable jobs    render owner
                                            ↓
                                prepare → submit → present

audio thread ← bounded audio commands
```

Один авторитетный владелец клиентского session-state. Графический backend принадлежит render owner; главный поток остаётся совместим с требованиями window system. На системах, где render должен жить рядом с event loop, совместить их без переноса тяжёлых jobs туда. Audio callback не блокируется locks/allocations/IO.

Сетевой IO — async runtime. CPU decoding/baking/meshing — ограниченный пул; не запускать CPU-bound jobs как бесконечные async callbacks. Пул делит бюджет между meshing, decompression, asset bake, WASM compilation и тестовыми captures. Приоритет — input/session, видимая геометрия, ближайшая необходимая геометрия, затем prefetch. Количество workers не равно автоматически числу всех logical CPUs.

Клиентская симуляция и сетевые действия не привязаны к render FPS. Базовую частоту и правила ускорения/freeze/step/tick-rate packet закреплённой версии воспроизвести отдельно. Для vanilla-события, у которого нет server tick ID, не выдумывать его: хранить packet sequence, client tick и время наблюдения.

Не делать неограниченный catch-up после торможения; накопление долга имеет метрику и bounded policy. Render может пропустить промежуточную презентационную версию; gameplay state и необходимые network effects пропускать нельзя.

### 5.3. Representation и lifetime

`ServerObservedState` хранит принятые server facts. `PredictedLocalState` содержит только разрешённые локальные предсказания и основания их reconciliation. `RenderScene` — производная презентация; `UIModel` и MCP observations — bounded projections.

Предсказания не становятся authority по истечении таймера. Подтверждение/отмена сопоставляется с событиями протокола. Renderer не изменяет мир. WASM не записывает здоровье/инвентарь. MCP не обходит собственный action executor.

Для block prediction presentation block entity следует локальному блоку: predicted air
немедленно убирает её из render/collision store, а predicted placement создаёт
обычную constructor projection. До ACK отдельно сохраняется полученная block entity
(включая NBT и локальные component/text projections), чтобы rejected break не
создавал пустую замену. Block-entity data до ACK обновляет соответствующую
полученную сущность даже при временном predicted air; смена server block type
сбрасывает данные прежнего типа.

Корректный command-suggestions packet не становится ошибкой соединения, если
подстановка не помещается в лимиты chat editor. Такая подстановка — атомарный
локальный no-op: текст, cursor, selection и composition не меняются; другая
подсказка автоматически не выбирается. Некорректный wire packet остаётся ошибкой.

Каждый async job несёт минимум:

```text
process_instance_id
connection_epoch
dimension_epoch
chunk_or_entity_generation
source_revision
resource_pack_epoch
relevant_neighbor_or_light_revisions
```

Проверять только релевантные зависимости, а не инвалидировать весь мир из-за любой мелочи. При смене сервера или измерения устаревший результат не может приклеиться к чанку с теми же координатами. Identity сетевой сущности включает lifetime generation, а не один переиспользуемый numeric ID.

Не копировать целый world snapshot каждый кадр. Использовать изменившиеся секции/компоненты, immutable handles и короткие presentation buffers. При пропуске consumer delta восстановиться из ограниченного актуального снимка; не применять половину потерянной цепочки deltas как валидное состояние.

### 5.4. Данные мира

Хранить chunk sections в compact palette/bit-packed representation, uniform/empty sections без развёрнутых массивов. Numeric registry IDs — runtime/session-scoped; namespaced identity — для долговременных ссылок. Flags материала/shape/opacity/render-layer предвычислять при activation registry.

Разделить block storage, lighting, biome data, block entities, mesh residency и visibility. Разгрузка GPU mesh не означает, что сервер перестал считать chunk клиенту известным. Разгрузка packet-state по eviction не должна создавать невидимые препятствия для local prediction: потребовать повторное получение или ограничить допустимую область явно.

Сущности: dense generational slots, hot arrays для transform/pose/render bounds и sparse cold data. Не требовать полной ECS-библиотеки; structured SoA используется там, где есть измеряемые массовые обходы. Никаких строковых dispatch по имени типа в render loop.

## 6. Сеть, prediction и отсутствие второй серверной симуляции

### 6.1. Протокол

Для каждого packet ID в каждом состоянии описать направление, decoder/encoder, bounds, action/state effect и test. Frame length, VarInt, compression threshold, decompressed size, NBT depth, string/components limits, array counts проверяются до больших allocations. Шифрование/сжатие включается строго на предусмотренной границе, а не задним числом на уже накопленные bytes.

Порядок semantic packet application сохраняется. Дешёвая frame extraction может предшествовать parallel decompression/parse, но state application коммитится в исходном порядке. При reconfiguration/reconnect старые queues аннулируются по epoch. Unknown обязательный packet/registry приводит к диагностике несовместимости, а не silent skip; пропуск разрешён только там, где это допускает конкретный протокол.

Протокол не «zero-copy любой ценой». Входящие buffers можно переиспользовать и не копировать промежуточные представления; долговременное состояние должно владеть безопасными bytes/typed data, а не ссылками в возвращённый pool.

### 6.2. Локальная физика

Client movement parity требует собственного version-specific conformance набора. Похожая `mc-physics` серверная функция не считается автоматически правильной клиентской физикой. Повторно использовать проверенный общий kernel только при совпадении алгоритма и порядка операций.

Проверить все формы, friction, step-up, crouch edges, fluids, climbing, swimming, gliding, knockback, mounts, interaction reach, physics при низком TPS. Не включать fast-math/FMA/reordering в authoritative prediction без доказательства допустимости расхождения. Камера и GPU могут использовать другие числовые представления.

Network corrections применяются к канонической клиентской базе. Визуальное сглаживание отдельно и не скрывает collision/reach correction. Хранить число/величину коррекций; сравнивать на одних маршрутах с vanilla-клиентом.

### 6.3. Удалённые сущности

Клиент интерполирует/представляет полученные состояния, но не запускает серверный Brain, экономику поселения или damage resolution. Скрытый рендером NPC сохраняет актуальные сетевые данные. Позы/анимации могут вычисляться lazy, однако события и gameplay-relevant признаки не теряются.

Для Solaris допускается дополнительный negotiated animation/order presentation stream. Он не вводит вторую authority движения или здоровья и не становится обязательным для vanilla server.

### 6.4. Инвентарь и взаимодействия

Единый transaction/action path для человека, UI, MCP и разрешённых add-on intents. Учитывать active container ID/state revision, cursor stack, sequence, changed slots и серверную коррекцию. Reconnect/reopen инвалидирует старые slot handles. Click передан, click локально предсказан и инвентарь подтверждён сервером — разные состояния.

При attack/use/break completion фиксировать наблюдаемое условие. Удар не равен нанесённому урону, стрела не равна попаданию, исчезновение item entity не равно pickup именно этим игроком. В инструментах тестирования это различие сохраняется.

## 7. Ресурсная система и визуальная полнота

### 7.1. Asset pipeline

Два cache: immutable original bytes и disposable derived artifacts. Ключ derived cache включает source hashes, pack order, target game format, bake version, GPU format/capabilities и существенные renderer options. Изменение atlas layout, shader ABI или resource epoch не использует старый mesh вслепую.

Bake models/blockstates/item graphs один раз при смене ресурсов. Resolve parent references, cycles, variants/weights, multipart, rotations, UV lock, tint, ambient occlusion, cull-face. Для per-position random variants использовать version-correct seeds; порядок threads не меняет выбранную модель.

Пакетная обработка PNG/OGG/font не создаёт тысячу OS tasks. Общие decoded textures/models интернируются. Ошибки с exact owner/resource path выводятся в diagnostics; missing обязательный content не закрывается молча placeholder-кубом в приёмке.

### 7.2. Resource reload

`stage → validate → compile/bake → reserve GPU → atomic activate → acknowledge`.

До активации остаётся согласованная старая версия либо явный loading screen. Не рендерить new model с old atlas. На disconnect отменить фоновые downloads/jobs, завершить sounds, снять HUD/input bindings, освободить owner-scoped handles. Stale callback от предыдущего сервера отвергается.

Учитывать двойную память staging + active; reload должен заранее уложиться в budget или вернуть точную ошибку. Нельзя завершать handshake ACK до подтверждённой активации нужного content.

### 7.3. Геометрия и анимации мобов

Отдельный каталог реализованных vanilla render families: rig, layers, poses, equipment attachment points, held item transforms, baby scaling, projectiles, special entities и block entities. Ассеты задают только то, что фактически содержат; недостающие procedural transforms реализуются собственным кодом.

Один shared mesh/rig на family; экземпляр хранит transform, pose parameters, skin/material handles, equipment references. Возможны instancing и GPU skinning; не строить кубики модели каждого жителя каждый кадр. Layer combinations и shader pipeline states кэшируются.

Предмет в руке, выпущенная стрела, поднятый щит, повреждение и смерть должны отображаться согласованно даже в performance profile. Crowd rendering не превращает воинов в одинаковые billboard-силуэты рядом с игроком.

## 8. Рендерер: базовый контракт

### 8.1. Выбор backend

Стартовый renderer — `wgpu`; `winit` отвечает за desktop window/input [S05, S06]. Конкретные версии pin на C00 и проверить лицензии/MSRV/platform support. Native backends выбираются по реальным features/limits, не по названию GPU.

Режимы возможностей:

- **Portable:** обычные vertex/index buffers, CPU conservative visibility, instancing, bounded draw lists.
- **GPU-driven:** compute culling, indirect draws/compaction и multi-draw только при поддержке нужных features.
- **Advanced:** mesh shaders/дополнительные hardware features после независимого spike и A/B.

Это разные механизмы исполнения одного scene/material contract, не три самостоятельных движка. В актуальной документации wgpu есть native indirect-count и experimental mesh-shader features; проверять доступность на устройстве и зрелость реализации, а не повторять устаревшее «wgpu этого не умеет» [S07].

Если wgpu действительно ограничивает измеренный hot path, ADR должен показать минимальный repro, долю затрат, альтернативный prototype и цену поддержки. Затем возможна локальная native optimization или замена одного backend. Заранее писать собственный Vulkan + Metal + DX12 RHI не требуется.

### 8.2. Кадр

Явный небольшой render graph:

```text
consume scene changes
→ visible-set / GPU culling
→ opaque terrain + opaque entities
→ cutout materials
→ translucent world/entities/particles
→ overlays / hand / HUD / UI
→ optional presentation effects
→ present
```

Порядок отдельных passes уточняется vanilla oracle: sky, water, hand, outline, weather и эффекты нельзя расположить ради красивой схемы вопреки правильному blending/depth. Depth prepass и HZB не являются безусловным default на всех GPU: они принимаются по total GPU time, не только по снижению числа shaded triangles.

Никаких per-frame `device_wait_idle`, синхронных GPU readbacks, recompile shaders, realloc всех buffers или rebuild всего UI. Upload staging rings и resource retirement tied to submission completion. Frames-in-flight ограничены; throughput не покупается неограниченной input latency.

### 8.3. Числа и координаты

CPU world coordinates — точные integer block/chunk coordinates и достаточно точная локальная player representation. GPU — camera-relative f32 с region origin, не мировые f32 на миллионах блоков. Floating origin не меняет network coordinates. Tests: большие положительные/отрицательные координаты, смена origin, следование entity, particles и shadows.

Reversed-Z допустим при проверенной матрице depth и отсутствии артефактов всех passes. Precision compression/16-bit attributes вводить по явному диапазону; нельзя резать позицию или свет до неучтённой потери качества.

## 9. Terrain pipeline и радикальные оптимизации

### 9.1. Сначала уменьшить геометрию и подготовку

Кубические opaque blocks обрабатываются специализированным mesher: occupancy bitmasks, bulk neighbor tests, удаление внутренних faces, compact quad records. Complex block models используют общий baked-model путь. Оба пути дают совместимый material/mesh output.

Greedy merging разрешён только при равенстве всех условий видимой поверхности: material/texture transform, orientation, tint, render layer, AO/light interpolation, model semantics и корректного повторения UV. Нельзя склеить блоки с разным светом в один однотонный прямоугольник или растянуть текстуру. Диагональ quad, seam и interpolation на границе проверяются oracle-тестами. Если равенство не доказано — оставить больше quads.

`meshlet`/packed quads/vertex pulling — кандидаты на сокращение bandwidth. Не объявлять выгодным формат, пока учитываются shader decode cost, cache locality и memory. Формат описан и тестируется CPU decoder/reference shader; случайная bit packing ошибка не прячется только на одной GPU.

### 9.2. Инкрементальная перестройка

Block edit dirty-ит минимальную зависимую область: свою mesh tile/section и необходимые соседние границы. Light-only update, biome tint, transparency sorting и смена модели не обязаны всегда запускать один полный remesh.

Для первого рабочего пути section granularity допустима. Если бурст edits делает её дорогой, использовать независимые subtiles и заменить только затронутые fragments. Это доказанная оптимизация конкретного hot path, не обязательная заранее иерархия из пяти уровней.

Очередь meshing дедуплицируется по `section identity + newest revision`. Устаревшие ещё не начатые jobs заменяются; выполняемый job может завершиться, но stale result не коммитится. Job cancellation и resource-epoch invalidation обязательны. Приоритет близких видимых правок выше дальних новых чанков; нужен aging, чтобы фон не голодал бесконечно.

Маленькая ближайшая правка может получить дешёвый временный корректный patch mesh, пока строится оптимизированный результат. Нельзя показывать прежний целый блок после подтверждённого разрушения долго только ради batching. Временная геометрия использует те же material/shape правила; её lifetime fenced, никакого двойного face после основного commit.

### 9.3. Chunk streaming

Отдельные budgets: packet backlog, decompression, section decode, model lookup, mesh CPU, GPU upload, residency. Ограничение измеряется не только количеством chunks: два чанка различаются по bytes/complexity на порядки.

Приоритет по актуальной camera/velocity и необходимому collision neighborhood. Teleport аннулирует старый prefetch; не ждать выгрузки тысяч нерелевантных jobs. Дальние меши могут ждать, но клиент сообщает geometry-pending и не подтверждает complete scene. Ниже нагрузка за счёт обрезанного view distance не считается ускорением при сравнении.

Серверное управление chunk flow/batch acknowledgement, если оно есть в закреплённом протоколе, реализовать честно и метрифицировать. Не выдавать завышенную готовность, когда CPU/GPU queues переполнены.

### 9.4. Переиспользование света

Light arrays и biome colors хранятся отдельно. Проверить вариант выборки света в shader из обновляемого компактного representation вместо постоянного встраивания всей освещённости в mesh. Принимать только если он воспроизводит нужное AO/interpolation, уменьшает total cost и укладывается в VRAM/bandwidth.

Смена времени суток обычно меняет общие lightmap/sky parameters, а не geometry каждого чанка. Dynamic lights от предметов — отдельная расширенная опция, если не являются vanilla baseline. Нельзя незаметно включить более дорогую графику и затем сравнить её с другим клиентом без этой функции.

## 10. Visibility, GPU-driven submission и толпы

### 10.1. Консервативное отсечение

Порядок решений: distance по выбранным settings → frustum → проверенные connectivity/occlusion → GPU HZB при выгоде. False positive означает лишнее рисование, false negative — пропавший объект; корректность предпочитает первое.

Occluder не равен любому non-air block. Glass, leaves/cutout, fences, частичные shapes, translucent surfaces не становятся полным непрозрачным кубом. Структуры со смешанной прозрачностью имеют conservative bounds. Spectator, камера внутри geometry, underground, portals, glows/nameplates и breaking outlines проверяются отдельно.

Предыдущий depth buffer не является доказательством текущей невидимости после поворота камеры, разрушения стены или появления объекта. Для disocclusion использовать current-frame recheck/validity bounds, near-zone visibility и invalidation. Parity profile не принимает мерцание/появление моба с задержкой как бесплатную оптимизацию.

### 10.2. GPU-driven путь

Общие mesh/material pools, compact instance data, GPU-visible draw descriptors. На подходящем устройстве compute строит/компактизует visible draw lists; indirect draw count остаётся на GPU. Не читать количество draw calls обратно на CPU каждый кадр.

Khronos публикует пример GPU-generated indirect rendering с устранением повторных CPU bindings [S10]. Здесь принцип применяется к terrain sections, повторяющимся моделям и equipment. Не утверждать заранее кратность ускорения: измерить CPU submission, GPU culling, raster и bandwidth по отдельности.

Features/limits (`indirect`, draw count, storage buffers, texture arrays/binding arrays и т. п.) проверяются явно. Portable путь использует те же descriptors и materials, меняется только способ формирования/исполнения draw list. Device loss или недоступная feature дают диагностированный fallback, а не чёрное окно.

### 10.3. Entities и animation sharing

Рендерить одинаковую topology/rig/material-family инстансами. Разделить transform update, pose evaluation, skinning и draw submission. Shared animation samples для одинаковых clip/time buckets допустимы при заданной точности; индивидуальное equipment, hit animation и идентичность остаются.

Near entities и важные события обновляются полноценно. Для дальних декоративных анимаций возможна отдельная presentation LOD с интерполяцией, только в явно выбранном профиле. Нельзя снижать network state consumption, локальную collision/hit interaction или терять сигнал поднятого оружия/щита.

Огромный roster на сервере не означает, что клиент обязан рисовать 100k humanoids в одном кадре, и успех кэша 100k state-records не является доказательством такой отрисовки. Раздельно измерять received, client-known, visibility-eligible, submitted, rasterized/covered и animated entities. Culling не должен скрывать реально видимую нагрузку ради бенчмарка.

Equipment attachments и block entities часто дают много мелких draws: они входят в batching, но типоспецифичные состояния не сводятся к generic cube. Names/tags/UI markers виртуализируются по видимости и масштабу, с сохранением необходимых целей управления.

### 10.4. Mesh shaders

После стабильного GPU-driven indexed path провести отдельный spike: representative terrain, GPU counters, latency, power, memory и AMD/NVIDIA/Intel/Apple coverage. Сравнивать одинаковые scenes и visual quality. Если нет end-to-end выгоды, advanced path не становится обязательным.

Не делать NVIDIA-only основой клиента. Не связывать mesh shaders с наличием RTX branding. Nvidium полезен как пример специализированного backend, но его аппаратные ограничения нельзя переносить на весь продукт [S09].

## 11. Прозрачность, текстуры, particles и память GPU

### 11.1. Прозрачность

Opaque, cutout, translucent и additive материалы различаются. Проверить воду за стеклом, несколько слоёв tinted glass, fluids, particles, entities, item layers и camera-inside-fluid. Прозрачные quads сортируются/обрабатываются по semantics выбранной версии; непрозрачный batching не должен ломать этот порядок.

Incremental sorting при изменении камеры/геометрии предпочтительнее полного sort всех transparent primitives каждый кадр. Сложные циклические пересечения получают определённое поведение, сопоставленное с oracle.

Weighted blended OIT может дать эффект, но меняет картинку. В `Parity` не включать его как эквивалент без доказательств. Approximate OIT относится к явно обозначенному visual mode. «Стекло в основном выглядит нормально» не закрывает прозрачность.

### 11.2. Texture/atlas strategy

Atlas или arrays выбираются по характеристикам pack и GPU. Обеспечить mip padding, границы tiles, UV repeat, anisotropy, alpha cutoff и корректное sRGB/linear blending. Сортировка textures по размеру не должна ломать animated sprites. Анимации texture обновляют необходимые tiles, а не весь atlas.

Предварительно транскодированные GPU-compressed textures — optional derived cache. Original PNG сохраняется как source; lossy compression в parity не выдаётся за точную. Lossless/statically equivalent memory сокращение допускается. Для high-res packs есть отдельный declared memory budget и понятная ошибка при превышении.

Bindless/binding arrays не являются условием работы на всех устройствах. Не создавать descriptor на каждый block instance.

### 11.3. Particles/weather

Пулы, packet/event-driven spawning, bulk update, GPU sim только для чисто визуальных частиц. Поддержать collision-sensitive particles и нужные trajectories, emitter lifecycle, underwater/weather effects. CPU/MCP не обязан читать каждую частицу обратно с GPU.

Reduction spawn count/distance — graphics setting, не невидимая оптимизация. В equality-бенчмарке particle setting и seeds совпадают. Для массового боя фиксировать активные projectiles отдельно от декоративных particles: снаряд нельзя удалить по particle budget.

### 11.4. Residency и фрагментация

GPU arenas для terrain, entities, textures и staging. Видимые pinned allocations защищены, дальние/устаревшие evict-ятся. Fragmentation, reserve/used bytes и eviction churn измеряются. Defragmentation инкрементальна, не `stop-the-world` на render thread.

На reload/teleport лимит учитывает active, retired и staging allocations. Освобождение только после нужного GPU completion. При pressure сначала сокращать предзагрузку и удалять невидимые derived resources; изменение качества/дальности — только объявленная политика. OOM не приводит к циклу бесконечной повторной загрузки одного ресурса.

Для UMA не складывать RAM и VRAM как независимую физическую память: показывать метод измерения. Budget учитывает backend/driver overhead; невозможность получить точный resident VRAM не маскировать выдуманным числом.

## 12. Профили качества и критерий «радикальной оптимизации»

### 12.1. Три отдельные группы

**A — эквивалентные оптимизации:** удаление внутренних faces, валидное culling, batching, caching, incremental work, memory layout, асинхронная подготовка, GPU submission. Должны сохранять поведение и оговорённую точность картинки.

**B — явно выбранные визуальные компромиссы:** geometry LOD, дальние animation buckets, impostors, уменьшение particles, dynamic resolution, upscaling, approximate transparency, дальний landscape cache. Они не меняют authoritative gameplay, но могут менять видимость/читаемость; тестируются отдельно.

**C — недопустимая подмена результата:** меньше видимых мобов при тех же settings, пропуск взаимодействий/пакетов, уменьшение simulation tick rate, телепорты вместо движения, omission сложных моделей, скрытая смена render distance. Такие изменения не закрывают performance gate.

### 12.2. Настройки

`Parity` — обязательный эталонный профиль с зафиксированными существенными rendering semantics. `Performance` — только явно раскрытые компромиссы. `Enhanced` — дополнительные эффекты, не участвующие в сравнении vanilla parity. GPU feature tier не равен visual preset: optimized renderer должен работать и в Parity.

Dynamic resolution имеет видимый диапазон/текущий scale и по умолчанию отключена в gates. Upscaling/frame generation не считается улучшением simulation latency или native-render FPS. Distant terrain нельзя вычислять из скрытого server seed или отображать неизвестные chunks; cache уже виденного мира имеет stale policy и не используется для gameplay raycast/серверной разведки.

Профили и auto-quality не меняют настройки во время зафиксированного бенчмарка. Любое адаптивное решение записано в receipt. Не поддерживать десятки безымянных experimental flags в production; успешный fast path становится обычным кодом, неуспешный удаляется.

### 12.3. Приоритет экспериментов

| Очередь | Оптимизация | Подтверждающий показатель |
|---|---|---|
| 1 | No full-world/full-population rebuild per frame | objects scanned, bytes copied, allocations/frame |
| 2 | Incremental meshing/dirty dedup | CPU ms per committed mesh, stale jobs, edit-to-visible latency |
| 3 | Shared meshes/materials/instancing | draw calls, CPU submission, instances/draw |
| 4 | Conservative section/entity culling | rejected work, correctness diff, GPU time включая culling |
| 5 | GPU indirect compaction | CPU vs GPU contribution, отсутствие readback stalls |
| 6 | Compact quad/vertex formats и light separation | upload bytes, VRAM, decode/shader cost |
| 7 | SIMD hot loops | реально векторизуемая доля и full-scene эффект |
| 8 | Mesh shaders/advanced backend | end-to-end преимущество на нескольких scenes/devices |
| 9 | Optional LOD/upscaling | отдельный quality/performance frontier, не смешанный результат |

Проценты ожидаемого ускорения заранее не подставлять. Для решения использовать профиль и Amdahl: ускорение маленькой части кадра не даёт такую же кратность всему кадру. CPU frame preparation, GPU duration и present wait перекрываются; их перцентили не складывать.

## 13. Интерфейс, звук и OS integration

### 13.1. UI

Основной UI использует retained semantic tree + dirty layout/paint и виртуализированные большие списки. Стиль сохраняет узнаваемость и функциональность Minecraft; отдельная тема допускается. Debug overlay может использовать готовую библиотеку, но не создаёт второй authoritative inventory/action path.

Каждый интерактивный элемент имеет stable semantic ID, роль, label, enabled/deny reason, bounds, focus, owner namespace, screen instance и revision. MCP и accessibility читают эту модель, а не альтернативный список «якобы существующих кнопок». Click semantic target проходит реальную проверку текущей видимости, focus и доступности.

Поддержать scaling/DPI, window resize, fullscreen, alt-tab, clipboard с согласованной policy, IME/composition, keyboard navigation, focus traversal, cursor capture/release, text selection, международный ввод. Offscreen logic-test не подтверждает работу Windows IME/Wayland pointer lock.

Resource reload не уничтожает введённый текст без причины; screen/session change корректно прекращает held actions. Локальный emergency-stop имеет приоритет над любой автоматизацией.

### 13.2. Audio

Native audio mixer с bounded queues, streamable music, positional voices и category controls. События звука привязаны к game/session epochs, а не к render visibility. Невидимый источник может быть слышен; culling его геометрии не отключает звук.

Проверки audio routing/decoded bytes не доказывают слышимый output. Нужны loopback/device captures в интеграционных tests, с различением real-device, virtual-device и null-sink. Смена устройства, unplug, volume zero, repeated join/disconnect и sound stop покрыты.

### 13.3. Доступность и приложения ОС

Subtitles, ремап клавиш, contrast, снижение визуальных раздражителей, narrator/text semantics, размер UI и цветонезависимые маркеры. Auto-open URL или clipboard write из server text запрещены без нормального пользовательского действия и policy.

Нативная process lifecycle: закрытие окна отменяет задания, сохраняет settings атомарно, disconnects, завершает child singleplayer server корректно. Device lost/minimize/suspend/resume не создают новые игровые session IDs без действительного перехода. Настройки аккаунта и renderer профили разделены.

## 14. Нативная поддержка Solaris

### 14.1. Сначала совместимость с существующим Loader

В исследованном рабочем дереве Solaris Loader закреплены **wire protocol 3 / artifact index schema 2**, а `LoaderPlatform` перечисляет `fabric`, `neoforge`, `forge` [R01, R02]. Rust-клиент не должен притворяться Fabric.

Нужен согласованный native-client platform/capability handshake: сервер явно знает поддержанные features, форматы bundles и presentation contracts. Это может потребовать версионированного расширения manifest/ack; номер и backward compatibility решаются по фактическому codec. Старые неизвестные поля нельзя начать принимать «ради совместимости» без обновления схемы.

Не ломать существующие vanilla+Loader клиенты. Поддержать общий capability контракт и shared authored bundles там, где они действительно portable. Контент должен объявлять необходимые возможности, а не автоматически считаться подходящим Rust-клиенту из-за наличия ZIP.

Реализовать permission prompt, exact origin identity, content hashes, лимиты, verified archive, immutable activation, view/HUD lifetimes, input bindings, world selection/preview, custom item/block presentation и sound. ACK означает активацию. Отказ не скачивает запрещённое и не создаёт частично активный registry.

В текущем дереве view keys стали namespaced: `minecraft_request_loader_view` принимает точный `view_key` [R03]. Не возвращать продуктовый enum `settlement/army` в общий client-core и не использовать старую документацию как источник истины.

### 14.2. Carriers и расширение реестров

Существующие ограниченные carriers — compatibility detail старого протокола. Начальный native adapter выполняет его семантику, если это требуется сервером. Для полноценного custom registry нужен отдельный namespaced-to-session-runtime mapping, version/hash checks и server authority; большие лимиты не появляются автоматически из нативности.

Нельзя «снять 128 items/8 blocks» только на клиенте и ожидать, что сервер/Loader codecs это поддержат. Новый capacity профиль утверждается с server/client validators, aggregate quotas и negative tests. Canonical world/inventory IDs не зависят от локальных GPU handles или порядкового номера загрузки.

### 14.3. Серверный и клиентский WASM — разные артефакты

`solaris:plugin` API 0.7 — существующий серверный компонентный контракт. Его модуль остаётся на сервере: не скачивать и не исполнять его повторно в клиенте. Нативная совместимость с WASM-аддонами сервера прежде всего означает правильную реализацию согласованных presentation/input/data surfaces.

Для полноценной клиентской расширяемости ввести **отдельный versioned WIT world** с рабочим именем `solaris:client`. Один продукт может поставлять `server.wasm`, `client.wasm`, assets и schema, но каждое исполняется своим host, с независимыми capabilities и секретами.

Сервер не пересылает клиенту свою private economy/AI state. Клиентский extension получает только данные, доступные этой сессии и разрешённые permissions. Изменение серверного состояния — typed request к серверному владельцу, никогда local authority.

## 15. Client-WASM: контракт и изоляция

### 15.1. Что расширению разрешено

Типизированные события lifecycle/UI/client-visible state deltas; создание bounded UI, HUD и scene attachments; declarative animations, bounded sounds, локальное namespaced сохранение настроек, отправка согласованных plugin actions на сервер.

World reads только из доступного клиенту состояния и с указанной областью/лимитами. Нет автоматического чтения всех chunks/frame. Подписки на конкретные события/изменения предпочтительнее guest polling. Input interception регулируется context/focus и не перехватывает emergency-stop/auth/consent.

Рендеринг предоставляется declarative host commands: mesh/material definitions прошедших валидацию ресурсов, instance updates, layout. Не выдавать WebAssembly raw GPU handles, произвольный command buffer, shell, HTTP client, filesystem root, tokens, process APIs или доступ к MCP service.

### 15.2. Исполнение

Wasmtime — исходный кандидат runtime, version pinned и security-tested [S11, S12]. Отдельные Store/instance и memory limits на extension, общий bounded worker budget. Нет invocation на render thread; декларативные curves исполняются native renderer без обязательного guest callback каждый кадр.

Pilot limits как исходные инженерные настройки, а не вечные ABI-ограничения: 32 MiB linear memory на компонент, 256 MiB aggregate по активным компонентам, bounded tables/stacks/instances, output batch limits, fuel на вызов, aggregate host-work quota. Конкретные fuel/host-call числа установить representative workloads до production. Native rendering ресурсов учитывается отдельно от guest memory.

Epoch/fuel ограничивают исполняемый WASM, но не блокирующий host call [S11]. Каждый host import имеет bounds, cancellation и native execution budget; никаких бесконечных IO/CPU loops внутри функции, «защищённой fuel». Ограничить большой `memory.copy` размером памяти/операции. Infinite loop или trap одного аддона не вешает кадр и не портит registry остальных.

Compilation/AOT происходит вне кадра. Deserialize precompiled executable cache только из собственной доверенной локальной compilation цепочки с проверкой engine/config/target и tamper protection; нельзя deserialize присланный сервером «ускоренный Wasmtime cache» как обычный asset [S13].

### 15.3. Permissions и resource lifetime

Manifest capabilities должны быть объяснимы пользователю. Первая активация/расширение полномочий требует согласия; hash подтверждает bytes, не добросовестность. Publisher signature, если добавляется, не заменяет sandbox и consent.

Каждый handle связан с owner, component generation, connection epoch и resource epoch. Reload/reconnect инвалидирует старые handles. Нельзя одним addon owner изменять чужой HUD или сохранять скрытый callback после удаления.

При trap компонент переводится в disabled/quarantined с причиной; клиент сообщает серверу negotiated failure, если от него зависел обязательный контент. Не повторять бесконечно падающий guest каждый frame. UI показывает какой addon потребляет CPU/memory/host work. Restart component не дублирует уже отправленную серверную операцию.

### 15.4. Независимый внешний код

Raw user/server shaders не включать в базовую sandbox-модель. GPU shader может зависнуть вне возможностей WASM fuel. Для первой версии — только проверенный набор native shader families и ограниченные material parameters. Пользовательские shaderpacks/compute доступны позднее через отдельный trust model, validation и device-failure handling; их нельзя назвать безопасными только по WGSL parser success.

Обновление client-WASM ABI сопровождается migration и совместимыми тестовыми компонентами. Не выдумывать cross-plugin service bus, пока нет конкретного сценария. Extension может расширять своё UI, но не динамически переписывать descriptions/схемы доверенных MCP tools.

## 16. MCP как первоклассный интерфейс клиента

### 16.1. Цель и reuse

MCP обязателен с первых этапов, не как поздний debug-addon. Его потребители: Opus/Codex/другие агенты, canonical harness, человек-разработчик и CI. Он должен позволять пройти функциональные игровые циклы, проверить UI и rendering, воспроизвести ошибку и собрать evidence.

В текущем `solaris-loader` уже есть реальные инструменты `minecraft_observe`, bounded reads/waits, movement/interactions, containers, scenarios и screenshot [R03, R04]. Новая реализация переиспользует **контракт и tests**, но работает напрямую с Rust-клиентом. Не переносить Java reflection и JVM bridge в native runtime. Не создавать отдельного bot-client, который отправляет packets вместо реального контролируемого игрока.

До changes экспортировать tool catalog текущего Loader, включая input/output schemas и semantics, в compatibility fixture. Старые имена и схемы, реально используемые harness, сохраняются thin adapters к новому typed action API. Не обещать совместимость только по совпадающим именам. Изменение completion semantics считается API change.

### 16.2. Версии протокола

Локальный `McpHttpServer.java` использует 2025-11-25 и поддерживает 2025-06-18 [R05]. На дату исследования официальный `/specification/latest` указывает **2026-07-28** [S14]. Новая спецификация изменяет lifecycle/metadata/cancellation; нельзя механически смешать новый URL с прежним initialize/session protocol [S15–S18].

План:

1. `Legacy adapter`: точная совместимость 2025-11-25 с нужным существующему harness lifecycle; 2025-06-18 только если это подтверждённый consumer.
2. `Current adapter`: 2026-07-28 с требуемой per-request metadata и правилами transport/versioning, отдельными conformance tests.
3. Общие tools/resources/actions за adapters, один клиентский state owner. Не дублировать игровые реализации.

Использовать официальный Rust SDK, если выбранная pin-версия покрывает нужные revisions/features [S19]. Если нет — небольшой явный protocol adapter с conformance coverage, не неподтверждённый claim «latest supported». Идентификатор MCP transport session и идентификатор игрового connection — разные вещи. Игровой `instance_id/connection_epoch` передаётся явно, особенно в протоколах без connection-scoped state.

### 16.3. Transports и запуск

Поддержать локальный stdio и authenticated Streamable HTTP. Предлагаемые будущие CLI:

```text
solaris-client play --profile <profile>
solaris-client play --profile <test-profile> --automation --mcp-http 127.0.0.1:0
solaris-client mcp attach --instance <instance-id> --stdio
solaris-client mcp config --instance <instance-id> --format <host-format>
solaris-client doctor --json
solaris-client test run <scenario-id> --artifact-dir <controlled-dir>
```

Это проектные команды. `mcp attach` — тонкий локальный transport bridge к уже работающему клиенту, без своего мира/игрока. Запуск тестового клиента — отдельная явная команда, не скрытая побочная реакция на `observe`.

В stdio только MCP JSON-RPC идёт в stdout; logs — stderr/file. HTTP по умолчанию loopback, случайный свободный port или заданный оператором, authentication, Host/Origin validation, body/connection/rate limits. Не открывать `0.0.0.0` автоматически и не создавать cloud tunnel из игрового клиента.

Remote доступ только явным конфигом оператора, через защищённый transport и корректный auth model. Для interoperable HTTP authorization реализовать требования выбранной спецификации; локальный pre-shared credential не выдавать за реализованный OAuth flow [S17].

### 16.4. Как агенты узнают о клиенте

При opt-in automation клиент создаёт owner-readable runtime descriptor: process instance ID, PID/start time, endpoint, supported protocol/tool versions, profile ID и public readiness. Secret хранится отдельно с OS ACL. Файл не подтверждает живость: attach сверяет nonce/process instance через endpoint.

Сделать:

- `AGENTS.md` с маршрутом к `docs/AGENT_TESTING.md` и правилами подключения;
- MCP resource `solaris-client://guide` с короткой инструкцией и примерами;
- `solaris-client://capabilities` с фактическими renderer/automation/target features;
- `solaris-client://instances`, `.../scenarios`, `.../limits`, `.../compatibility` с bounded pagination;
- generated host-config examples, smoke command и schema documentation из одного typed source;
- metadata/descriptions tools, описывающие side effects, scopes, limits и точное условие успеха;
- MCP prompts/guide integration в форме, соответствующей negotiated protocol.

Не обещать, что агент узнает tools без регистрации MCP в своём host. Setup выдаёт готовую конфигурацию; изменение чужих глобальных agent settings требует явной команды пользователя. Поддерживаемые host formats проверяются интеграционно. Проект не требует OpenAI/Anthropic API key для работы клиента или MCP.

### 16.5. Scopes и управление

Automation выключена в обычном пользовательском запуске, пока пользователь не включит её явно. Видимый индикатор показывает активного controller и scopes. Default session позволяет observation; управление включается локальным разрешением/тестовым профилем.

Scopes: `observe`, `control`, `ui`, `capture`, `performance`, `scenario`; отдельно `test_debug`. Game server или WASM addon не может выдавать scopes. Новый опасный scope не следует из старого согласия на HUD.

Один writer lease на player input; readonly calls могут быть параллельными. Lease имеет bounded TTL, controller identity и fencing token. Ручной ввод может отобрать управление; emergency-stop всегда прекращает held keys, macro и navigation. Смена screen/focus/connection, timeout, error, cancel и lease expiry освобождают все удерживаемые действия.

Нельзя «получить ещё одного контроллера» через второй MCP connection и одновременно нажать противоположные кнопки. Командование несколькими клиентами работает через разные instance IDs, а не глобальную mutable selected-instance переменную.

### 16.6. Никаких скрытых полномочий

Через обычный MCP нельзя выполнять shell, произвольный Lua/JS/Rust, читать файлы вне artifact roots, получать токены, менять server state напрямую или делать teleport. Setup/cheat hooks для собственной лаборатории — отдельная test-only surface, помеченная в evidence, недоступная обычному серверу и не засчитываемая как прохождение обычной игры.

Чат, книги, таблички, server MOTD и addon text — недоверенные данные. Их текст не становится system instructions, не расширяет список tools и не побуждает клиент автоматически менять auth/server/config. MCP возвращает source/owner и тип данных, а не исполняет содержащиеся команды.

## 17. Типизированная модель observations/actions

### 17.1. Observation envelope

Каждый ответ содержит применимые поля:

```json
{
  "instance_id": "opaque-process-id",
  "connection_epoch": "17",
  "dimension_epoch": "4",
  "state_revision": "18264",
  "last_applied_packet_sequence": "91608",
  "client_tick": "4092",
  "server_tick": null,
  "resource_epoch": "3",
  "captured_frame_id": null,
  "origin": "server_observed",
  "truncated": false,
  "next_cursor": null,
  "data": {}
}
```

64-bit counters передаются без потери точности JSON consumers: strings либо согласованный safe-integer формат. `server_tick=null` означает, что точного значения нет, а не ошибку транспорта. Rendering snapshot может иметь более старую `state_revision`; это явно отражается.

Одно observation — согласованный снимок, а не смесь inventory до packet apply и player pose после него. Дорогие projections bounded и возвращают pagination. `unknown/unloaded/not_authorized/not_supported` не превращаются в `air/empty/false`.

Отдельные понятия: `client_known` — получено/загружено клиентом; `in_frustum`; `line_of_sight_tested`; `submitted_to_renderer`; `covered_in_captured_frame`. Список всех loaded block data не называть тем, что игрок видит глазами. Debug scan явно отмечает более широкий loaded-world scope.

### 17.2. Action lifecycle

```text
requested → admitted → dispatched → locally_predicted
                               → server_observed / rejected / timed_out / cancelled
```

Не все действия проходят каждое состояние. Камера может завершиться локально; inventory transfer требует server-observed state; screenshot требует завершённого GPU capture конкретного frame. Вызов `present()` сам по себе не доказывает, что человек увидел изображение на мониторе.

Каждое mutating действие имеет `action_id`, `connection_epoch`, expected context и completion contract. Повтор с тем же action ID внутри bounded retention возвращает исходный status, а не повторяет click/attack. Один ID с другими args — error. JSON-RPC request ID не подменяет idempotency key.

Timeout после dispatch возвращает phase и известные effects. Агент сначала читает статус/наблюдения, не повторяет автоматически неидемпотентное действие. Restart клиента не перепроигрывает старую очередь input в новую игровую сессию.

### 17.3. Event-driven waits

Wait atomically подписывается на relevant producer events, проверяет текущее состояние и повторяет проверку при изменении. Закрыть race «условие стало true между read и subscribe». Нет polling каждого тика для неподвижного inventory. Duration-based input ждёт client clock; world facts — applied state events; кадр — render/capture events.

Wait принимает timeout, context fence, expected predicate и начальную revision. Bounded predicate DSL без arbitrary code: equality/range/existence/count и ограниченные conjunctions. Event buffer overflow даёт `cursor_expired`, после чего нужен новый snapshot, а не бесконечное ожидание потерянного события.

В C++/Java-like test description «sleep 5 секунд» заменять причинным wait. Но реальные external timeouts остаются safety limits. Если нужные ticks не идут из-за pause/freeze, вернуть причину `clock_not_advancing` по контракту; не подделывать продвижение мира.

### 17.4. Entity и UI references

Entity handle = connection/dimension scope + network ID + generation + доступный UUID/type fence. UI handle = screen instance + revision + semantic element ID. Container handle включает актуальный container/state ID и slot identity.

Stale handle после despawn/reopen/reconnect отвергается. Поиск «ближайший villager» допустим как discovery, но дальнейшее действие привязывается к найденной identity. Нельзя лечить/бить случайного нового NPC на месте прежнего.

## 18. Каталог MCP-возможностей

Ниже функциональные группы нового typed API. Public names уточняются при совместимости с существующим `minecraft_*` catalog; thin aliases не создают второго пути.

| Группа | Обязательная поверхность |
|---|---|
| Discovery | describe, capabilities, list instances, health/readiness, limits, schemas, guide |
| Sessions | connect, connection status/wait Play, disconnect; target profile и server identity явны |
| Observation | player/pose/stats, inventory/cursor, container, target/raycast, loaded blocks/light, entities, recipe book, effects, chat/components |
| Waits | state change, exact block, chunk-loaded, entity spawned/moved/removed, inventory condition, health, screen/view state, audio event, frame completion |
| Input | look/look-at, bounded semantic keys/mouse edges, hotbar, offhand, sprint/sneak, inventory open/close, release-all |
| Interaction | break/place/use/use-on/interact entity, attack once/hold where legitimate, drop, pick-block in permitted game mode |
| Containers | click/quick-move/drag/number-key/button, expected screen/container revision, server-observed result |
| UI | inspect semantic tree, focus/type/select/scroll/click, inspect deny reason, wait view, Loader action/request view |
| Navigation | bounded movement in known world using normal inputs; arrival predicate, progress/stuck reasons/cancel |
| Solaris | activated bundles/permissions, namespaced view/markers/selection, action receipt; only data sent to this session |
| Rendering | capture screenshot, inspect frame metadata/passes/draw stats, wait mesh/resource ready, optional bounded debug overlays |
| Audio | inspect playback state, start bounded capture from configured sink, read levels/artifact; distinguish command vs output |
| Performance | start/stop named capture, metrics snapshot, marker/timeline, memory residency, queues, shader/mesh compilation stats |
| Replay | start/stop bounded recording, load allowed replay, seek in local replay, export/redact; no arbitrary runtime file access |
| Scenario | list/run/status/cancel predefined or schema-validated local scenarios, assertions, evidence bundle |
| Controller | acquire/renew/release input lease, cancel action, emergency release; status доступен человеку |

### 18.1. Bounded defaults

Pilot defaults: `scan_blocks ≤4096 cells/request`, `list_entities ≤512/page`, UI tree ≤512 nodes/page, обычный JSON result ≤256 KiB, стандартный wait ≤120 s, один active movement controller. Большие данные — pagination/artifact resource, не мегабайты в tool text. Исторические 128-block/512-entity bounds существующего инструмента сохраняются там, где важны старым тестам [R03].

Эти лимиты относятся к **ответам/операциям**, не к количеству существующих объектов мира. Screenshot/video/audio имеют отдельные byte/time budgets. Cancellation должна работать во время ожидания и длительного capture, не после его окончания.

### 18.2. Navigation

Навигатор — convenience layer автоматизации: строит bounded маршрут только по разрешённым client-known данным и исполняет обычный input. Не пишет position, не посылает «идеальные» teleport packets, не игнорирует collision. Не использовать серверный seed, сохранение singleplayer или hidden core state.

Успех подтверждён actual local pose и требуемыми grounded/reach/line-of-sight условиями с допустимыми server corrections. `path_found` не значит `arrived`. Unknown terrain, blocked gate, fluid, cliff, moving target и stuck различаются. Return partial progress, blocked reason и observations, не ложный успех.

### 18.3. UI и согласия

Автоматизация обычных UI actions должна проходить те же validators, что пользователь. Отдельно запретить без local preapproval подтверждение account sign-in, выдачу новых addon permissions и опасных OS/network действий. В fixture profile заранее разрешены конкретные сервер/pack hashes/scopes; это отражено в run receipt.

Нельзя обходить disabled/hidden button прямым вызовом server action из MCP и считать UI работоспособным. Сочетать semantic assertions со screenshot checks: tree может быть верным, а реальная кнопка отрисована за экраном.

### 18.4. Capture и доказательства

Screenshot — реальный render target после нужных passes с frame ID/state/resource epoch, а не специально нарисованная test картинка. GPU completion, offscreen capture и display present фиксируются раздельно. Если window occluded/minimized и framebuffer не обновляется, это видно в результате.

Headless `state` не закрывает rendering; headless `offscreen GPU` не закрывает native window/input/presentation; реальный on-screen run обязателен для релиза. Pixel tests используют representative scenes, разрешённые masks/thresholds и отдельные проверки текста/порядка/геометрии. Нельзя одним огромным tolerance спрятать отсутствующие shadows, transparent layers или mobs.

### 18.5. Сценарии

Scenario — finite typed steps, seed/fixture, preconditions, allowed scopes, expected outcomes и cleanup. Никаких embedded shell/eval. Любой helper указывает, использовал ли normal input или privileged fixture setup. Сценарий имеет supported client/server profile, иначе status `not_applicable`, а не pass.

Пример содержания сценария `survival.obtain-log-and-craft`:

1. Подключить изолированного игрока и дождаться Play + необходимых chunks.
2. Зафиксировать initial inventory и доступное дерево в fixture.
3. Обычными inputs подойти, посмотреть на целевой блок, начать breaking.
4. Дождаться server-observed удаления блока и отдельно прироста нужного inventory.
5. Открыть реальный crafting UI, выполнить recipe, дождаться server-observed output.
6. Проверить cursor stack, итоговые количества, screenshot UI и отсутствие corrections/errors.
7. Release-all и cleanup только заявленного тестового профиля.

## 19. Надёжность MCP и агентская дисциплина

Сервер MCP работает с request concurrency, но все actions сериализуются по context/lease. Observe во время long-held input обязателен. Cancellation версии 2025 и 2026 не смешивать: в одной HTTP-разрыв не означает cancellation, в другой соответствующий stream close несёт иной смысл [S15, S20]. Application input lease остаётся дополнительной safety границей; завершение transport request не отменяет уже случившийся server effect.

Каждый tool имеет точный input/output schema, typed errors, redacted logs, scopes, side-effect annotation и examples. Mutation readOnly annotations должны быть правдивыми, но security не строится только на hints. Аннотации из чужого addon не считаются доверенными.

В `docs/AGENT_TESTING.md` зафиксировать обязательный рабочий цикл агента:

```text
describe/capabilities → выбрать exact instance/profile
→ baseline observe → action с context fence
→ causal wait → observed result
→ capture при визуальном изменении
→ assertions + evidence → release control
```

Руководство запрещает считать `dispatched` успехом, лечить timeout увеличением sleep, выполнять setup cheats в пользовательской игре, перечитывать весь мир и менять baseline после провала. Ошибка должна возвращать достаточно контекста для следующего осмысленного действия, не десятки тысяч строк лога.

Добавить генерацию skills/guide файлов для агентов, но не требовать, чтобы конкретная модель «запомнила» интерфейс из прошлой беседы. Новый agent session восстанавливает контекст из репозитория + MCP discovery. Contract documentation и actual tool schemas проверяются на рассинхронизацию в CI.

## 20. Singleplayer, сохранения и совместимость серверов

### 20.1. Локальный мир

Singleplayer запускает локальный Solaris как управляемый Rust child process с ограниченным IPC/control channel и обычным gameplay connection. Такая граница изолирует падение сервера и позволяет применять те же multiplayer tests. Встраивание server runtime в процесс клиента возможно позже только при реальном выигрыше; не создавать второй singleplayer engine.

Start/readiness/cancel/save/stop/crash recovery явны. Выход из мира сначала сохраняет авторитетный server state; forced close не обещает сохранение последнего кадра. LAN/remote players отключают обычную singleplayer pause по правилам выбранного режима. MCP wait сообщает, какой clock paused.

### 20.2. Нельзя выдавать Solaris-поведение за vanilla-parity

Совместимость сети не доказывает идентичный worldgen, mobs, redstone, loot или datapacks. `Singleplayer-Solaris` поставляется с честным именем ruleset. Для полного native vanilla singleplayer нужен независимый server parity gate на закреплённой версии. Использование официального Java server как локального oracle не закрывает отсутствие JVM в целевом runtime.

Импорт существующего мира только после backup/dry-run, schema/version validation и проверки реально поддержанных данных. Не открывать vanilla save на запись «попробовать», не регенерировать неизвестные sections и не удалять неизвестные entities/items. Неподдержанная область возвращает точную ошибку. Original остается неизменным при неуспешном импорте.

### 20.3. Multiplayer matrix

Минимум: официальный vanilla target server, актуальный Solaris без addons, Solaris со standard presentation bundles, Solaris с client-WASM и adversarial server fixture. Proxy/Paper-подобные серверы — отдельная declared interoperability строка при проверке, не «любой сервер» после одного join.

Realms, social/reporting и другие официальные службы требуют своего API/access evidence. При отсутствии доступа они остаются блокирующими соответствующий service-completeness claim; не заменяются неофициальной имитацией или отключением security. Продолжать реализацию остальных независимых частей, явно сохраняя этот gap.

## 21. Производительность и измеримые бюджеты

### 21.1. Что измеряется

Не «FPS в пустом мире», а frame-time distributions, input-to-client-update, input-to-submit/present latency, edit-to-visible, chunk-to-visible, network backlog, shader/mesh stalls, memory и power/thermal behavior. Wall-clock, CPU active time, GPU timestamps и VSync wait различаются.

Reference candidate по имеющемуся контексту проекта: Ryzen 5 7535HS, RTX 4050 Laptop 6 GiB, 32 GiB RAM, 1920×1080 native. Перед freeze gate подтвердить реальный hardware, GPU TGP/power profile, driver, compositor, refresh rate и охлаждение. Это проектный стенд, не заявленный здесь measurement.

Отдельные обязательные profiles: Windows/DX12; Linux/Vulkan; AMD/Intel capability baseline; Apple/Metal до полного desktop release. Не обещать одну цифру FPS на любом ноутбуке.

### 21.2. Предлагаемые целевые gates

Числа ниже — **цели разработки до первых измерений**, не прогноз и не доказанное преимущество. C00 фиксирует воспроизводимые scene fixtures/quality и при необходимости уточняет исходный контракт открытым решением; после начала A/B workload и границы не менять ради зелёного статуса.

| Gate | Workload | Целевое время кадра/граница |
|---|---|---|
| P01 Traversal | 1080p, Parity, view 16, normal particles, разнообразный terrain, обычные mobs, прогретый маршрут | p50 ≤8.3 ms; p95 ≤12.5 ms; p99 ≤16.7 ms |
| P02 Streaming | Тот же profile, новая местность/быстрое движение и teleport, отдельно cold caches | p95 ≤16.7 ms; p99 ≤25 ms; очередь bounded, stall >50 ms отдельно учитывается |
| P03 Battle | 1024 реально присутствующих бойца, 50 player avatars, terrain/оборудование/снаряды и звук, видимость зафиксирована | p95 ≤16.7 ms; p99 ≤25 ms |
| P04 Crowd stress | 4096 бойцов с репрезентативной экипировкой, та же камера/геометрия; точный visible-set | p95 ≤33.3 ms; p99 ≤50 ms; не удалять видимых участников |
| P05 Client-state scale | 100k полученных entity records, фиксированный subset 4096 visible-eligible, burst metadata | Bounded memory/queues; отсутствие O(100k) полного render rebuild/frame; это не 100k одновременно отрисованных тел |
| P06 Memory | P01 steady 30 min | Исходная цель process resident RAM ≤2 GiB и tracked GPU allocations ≤2.5 GiB; driver overhead отдельно |
| P07 Reload/soak | 20 join/disconnect + reload cycles, затем 2 h mixed play | Нет монотонного роста live resources; данные и hooks предыдущего сервера не остаются активными |
| P08 MCP idle | Тот же scene, endpoint on/off без запросов | overhead стремится к нулю; gate ≤1% median CPU frame и отсутствие периодических world scans |
| P09 MCP active | 10 bounded observations/s + 1 causal action/s, без heavy capture | p95 regression ≤5% на том же baseline; корректная cancellation и bounded queue |
| P10 Interaction | Близкий одиночный block edit и GUI action при рабочем FPS | Source-apply → first correct submitted frame p95 ≤50 ms; server/network latency измеряется отдельно |

Очень плотные scenes могут упираться в overdraw/pixels, а не draw calls. P04/P05 нельзя переносить на слабую integrated GPU без отдельного профиля. Неудача P03 не исправляется только увеличением целевого числа FPS в README.

### 21.3. Методика

Сравнения: unmodified vanilla target и совместимый Sodium target с exact versions/settings, плюс собственный no-optimization/reference path там, где он нужен для локального эксперимента. Нет требования распространять чужой мод вместе с клиентом. Если compatible baseline mod отсутствует, фиксировать `unavailable` и не подставлять другую Minecraft.

Для steady gates: минимум 5 повторов по 10 минут после одинакового warmup; cold/streaming отдельно. Хранить raw per-frame samples, p50/p95/p99/p99.9/max, count stalls, camera track, actual resolution, visible counts, content hashes, world mutations и settings. Отсутствующие/пропущенные frames входят в отчёт. Нельзя удалить худшие тики как «неинтересные» после просмотра результата.

Сначала isolated renderer replay, затем live server, затем co-located client+Solaris как отдельный сценарий. Стабильные frames не исправляют серверные 100+ ms ticks; network/server latency не записывать в GPU time. Конкурирующие builds, swapping, thermal throttle и CPU quota фиксируются.

Визуально эквивалентные сравнения не включают dynamic resolution, frame generation, урезанные particles или другую дальность. Сравнение Performance presets разрешено отдельно с точным описанием потери качества.

### 21.4. Счётчики до оптимизации

CPU: input/apply/prediction/scene extraction/visibility/mesh/decode/UI/submission ms, allocations/bytes, per-stage queues, stale/cancelled jobs, memory residency.

GPU: per-pass timestamps при поддержке, draw/indirect counts, submitted primitives/instances, upload bytes, mesh/texture residency, staging wait, retired bytes, pipeline compilation, present wait. Отсутствующий GPU counter обозначается unsupported.

Gameplay: corrections, packet backlog, inventory mismatches, entity lifetime reuse, dropped essential events, tick debt. MCP: request latency, projection bytes/cells, controller leases, wait wakeups, cancelled actions, capture overhead.

Counters агрегируются локально; не делать global lock/atomic/log на каждый vertex/entity. Heavy tracing/capture opt-in и не сравнивается с baseline, где tracing выключен. Отдельный diagnostic mode может быть дороже, но не должен называться release performance.

## 22. Безопасность, ошибки и обновления

### 22.1. Trust boundaries

Недоверенные входы: game server, downloaded resource pack, skin/image/audio/font, WASM module, replay, world import и внешний MCP caller. Validate на каждой границе: authentication → size/schema → ownership/lifetime → capability → game action. Уже распарсенный объект не считается автоматически семантически допустимым.

Auth material, game credentials, addon permissions и MCP credentials раздельны. Server bundle не может привнести admin tool или изменить hotkey аварийной остановки. Локально управляемый singleplayer server не получает доступ к vault всего клиента.

### 22.2. Failure policy

- Broken optional cosmetic resource: bounded error и documented fallback, без claim parity этого ресурса.
- Broken mandatory protocol/registry/required addon: отказ совместимости с причиной до применения неоднозначных действий.
- Network loss: release input, заморозить/очистить непроверенные pending effects по правилам; не продолжать «офлайн бой» как будто сервер подтвердил.
- GPU loss: остановить небезопасные submissions, сохранить session/input safety, диагностировать adapter; controlled renderer reinit либо выход с report.
- Disk full/corrupt cache: source/cache integrity error, atomic recovery; не перезаписывать рабочий профиль пустым.
- WASM timeout/trap: изолированное отключение компонента, scoped cleanup, понятная причина.
- MCP disconnect/cancel: bounded cleanup согласно version adapter и controller lease; уже committed effects не откатываются фиктивно.

Ошибки типизированы и имеют стабильные codes, stage, safe context и retryability. Не возвращать agent-у `unknown error` там, где известны `stale_screen`, `unloaded_chunk`, `permission_denied`, `server_rejected`, `render_not_ready` или `action_dispatched_result_unknown`.

### 22.3. Updates

Подписанные собственные release manifests, hashes, rollback и проверка схемы settings/cache. Не запускать обновление бинарника из server plugin. Автообновление не меняет pinned game/resource version внутри воспроизводимого теста.

Resource-derived caches можно сбросить; пользовательские saves/config/account bindings — нет. Unknown future settings сохраняются либо migration fail-closed; предыдущая рабочая версия остаётся доступна до успешного обновления.

## 23. Система доказательств и обязательные проверки

### 23.1. Независимость oracle

Четыре контура:

1. Pure Rust unit/property/fuzz tests с собственными fixtures.
2. Differential protocol/physics/UI-behavior tests против официального target-клиента/сервера.
3. Native real-client integration через MCP, включая actual render/audio.
4. Solaris integration и full campaign client scenarios.

Совпадение собственного клиента с собственным сервером может скрывать общую ошибку. Поэтому vanilla server и Java client oracle обязательны. Reuse общего physics kernel не отменяет независимую проверку. Если источник oracle мутирует fixture, разные клиенты получают копии исходного мира или последовательно воспроизводимое состояние.

Оригинальные ассеты/Java oracle не распространяются в public CI artifact. Public tests воспроизводятся на авторских fixtures; private/local parity harness скачивает разрешённые ресурсы сам и пишет их hashes.

### 23.2. Evidence bundle

Каждый run содержит `run.json`, exact build/tree fingerprint, dependency/compatibility lock, hardware/driver/settings, server profile/build, scenario seed, timestamps/clocks, input trace, relevant packet/applied-event trace, assertion results, raw metrics и ссылки на screenshots/audio/video. Secrets/личные chat data redacted перед экспортом. Redaction не выдаётся за byte-identical signed-chat fixture.

Для теста screenshot — frame ID + resource epoch + relevant state revision. Для inventory — before/after server-observed slots/cursor. Для navigation — actual pose track/corrections. Для rendering — submitted/presented/captured различаются. Для аудио — какой sink/device действительно записан.

Export только в разрешённый artifact root; MCP возвращает opaque resource URI. Не принимать произвольный путь записи из server message. Скриншот необязателен для каждой проверки количества, но обязателен для claim о визуальной поверхности.

### 23.3. Acceptance matrix

Все пункты ниже обязательны для соответствующего полнотного уровня; прохождение не заявлено этим документом.

| ID | Сценарий / ожидаемый результат |
|---|---|
| AUTH-01 | Собственный зарегистрированный клиент выполняет разрешённый sign-in, проверяет доступ и подключается к online-mode vanilla; credentials не попадают в MCP/log |
| AUTH-02 | Expired/revoked token, отсутствие доступа, user cancel и network error различаются; нет fallback на чужую identity |
| ASSET-01 | Clean profile получает нужные source assets, hashes/size совпадают, повторный запуск использует verified cache |
| ASSET-02 | Truncated/corrupt download и concurrent install не публикуют partially-valid cache |
| ASSET-03 | ZIP slip, duplicate paths, decompression bomb, oversized PNG/font/OGG и циклическая модель отклоняются bounded |
| ASSET-04 | Release archive и source tree не содержат запрещённых Mojang bytes; provenance/license manifest проходит review |
| NET-01 | Покрыты все состояния/переходы и обязательные packets закреплённой версии; не только Login+Play |
| NET-02 | Encryption/compression boundaries, malformed lengths/VarInts/NBT и limits покрыты fuzz/property tests |
| NET-03 | Reconfiguration, transfer/respawn/dimension change не допускают stale packet/job commit |
| NET-04 | Packet burst не вызывает unbounded memory, semantic reorder или silent drop необходимых событий |
| MOVE-01 | Набор stairs/slabs/doors/fences/fluids/ladder/honey/ice/edge-crouch проходит differential movement tests |
| MOVE-02 | Sprint/swim/crawl/glide/vehicle/mount/flight и server corrections дают version-correct поведение |
| MOVE-03 | Низкий/меняющийся FPS не ускоряет movement, break progress или input cadence |
| MOVE-04 | Большие координаты и origin shift не дают jitter/ложный raycast/network drift |
| INT-01 | Breaking/place/use/attack/main/offhand проходят ordinary input с server-observed результатами |
| INT-02 | Bow/crossbow/shield/fishing/food/bucket и отмена использования сохраняют корректные lifecycle/events |
| INV-01 | Все click modes, cursor stack, crafting и recipe book совпадают с oracle |
| INV-02 | Закрытие/повторное открытие контейнера и stale state ID не дают dups/ghost success |
| INV-03 | Все container screen families из compatibility matrix проходят реальные UI сценарии |
| ENT-01 | Каждый vanilla entity/block-entity family имеет проверенные geometry/metadata/pose/layers без обязательных placeholders |
| ENT-02 | Despawn+reuse numeric ID и dimension transition не переиспользуют stale animation/action handle |
| ENT-03 | Hidden-by-culling entity продолжает принимать нужный state; при появлении не теряет damage/pose/equipment |
| VIS-01 | Все block model families, rotations, multipart, tint, animations и item-model graphs отображены |
| VIS-02 | Greedy meshing не меняет UV, AO/light gradients, seams/diagonal и partial block visibility |
| VIS-03 | Glass/water/cutout/particles/entities в разных слоях проходят transparent regression scenes |
| VIS-04 | Camera teleport/fast turn/broken occluder/new entity не дают false-negative visibility |
| VIS-05 | Reload не показывает new mesh с old atlas; stale jobs и GPU handles отвергнуты |
| VIS-06 | Near edit-to-visible измерен под streaming load; patch/full mesh не дублируют faces |
| VIS-07 | Sky/weather/portal/underwater/hand/outline/glow и все HUD layers проходят scene checks |
| VIS-08 | Portable и GPU-driven paths дают согласованный результат на одинаковом visual profile |
| UI-01 | Все базовые menus/HUD/screens работают mouse+keyboard и имеют актуальное semantic tree |
| UI-02 | Cyrillic/CJK/bidi/IME, DPI scaling, resizing/fullscreen/focus и clipboard policy проверены на ОС |
| UI-03 | Disabled/hidden/stale UI actions не обходятся через MCP |
| AUDIO-01 | Positional/loop/music/category/mute/subtitles и disconnect cleanup подтверждены output capture |
| AUDIO-02 | Device switch/потеря устройства не блокируют client loop и не оставляют вечные voices |
| SOL-01 | Native platform объявлена честно; проверены manifest/ACK compatibility и отказ неподдерживаемых features |
| SOL-02 | Отказ permissions не загружает/активирует запрещённый bundle; changed scopes требуют нового решения |
| SOL-03 | Namespaced views/HUD/input/world selection/previews/items/blocks/sounds работают с реальными fixtures |
| SOL-04 | Смена сервера удаляет все owner-specific UI/bindings/sounds/handles без утечки между соединениями |
| WASM-01 | Server WASM не исполняется на клиенте; client-WASM имеет собственный проверенный WIT contract |
| WASM-02 | Infinite loop, allocation flood, host-call flood, trap, malformed module и большой copy bounded |
| WASM-03 | Guest не читает tokens/files/hidden state и не имеет raw GPU/MCP authority |
| WASM-04 | Reload/disable/crash не повторяют серверный action и не повреждают соседний addon |
| MCP-01 | Старый catalog/semantic contract проходит shared fixture tests, новые tools обнаруживаются через schemas |
| MCP-02 | stdio и HTTP проходят соответствующие protocol-version conformance tests без смешения lifecycle |
| MCP-03 | Unauthorized/origin-rebinding/body-limit/foreign-instance requests отвергаются |
| MCP-04 | Readonly observe работает одновременно с hold input; два controllers не получают одну write lease |
| MCP-05 | Cancel/timeout/focus-loss/disconnect/expiry release все keys и прекращают navigation |
| MCP-06 | Wait не пропускает событие между read/subscribe и не busy-polls idle world |
| MCP-07 | Один action_id не выполняется дважды; timeout после dispatch не возвращает fabricated success |
| MCP-08 | Snapshot consistency, epochs, pagination, unknown/unloaded и truncation отражены точно |
| MCP-09 | Agent проходит survival crafting и container scenario без privileged mutation hooks |
| MCP-10 | Screenshot подтверждает actual framebuffer; offscreen/state-only flags не выданы за onscreen evidence |
| MCP-11 | Startup descriptor/guide/host config позволяют новому агенту обнаружить exact instance без знания прошлой беседы |
| MCP-12 | Недоверенный текст в чате/книге не меняет permissions, tool catalog или actor instructions |
| SP-01 | Локальный Solaris start/save/exit/crash/restart и LAN выполняются без JVM клиента |
| SP-02 | World import сохраняет оригинал и явно отвергает неподдержанное вместо data loss |
| SP-03 | `Singleplayer-vanilla-parity` не объявляется без отдельного server/worldgen coverage |
| PERF-01 | P01–P10 имеют raw measurements, equality settings и неизменённые workloads |
| PERF-02 | Backend feature unavailable, GPU loss и memory pressure дают корректную degraded/error policy |
| PERF-03 | Многочасовой mixed run не накапливает handles, watchers, textures, wasm stores и input leases |
| REL-01 | Чистая сборка/установка на целевых ОС, правильные licenses/signatures и pinned dependencies |
| REL-02 | Каждый unsupported service/pack-extension/version виден в compatibility matrix; нет полного claim с пустыми обязательными строками |

### 23.4. Стресс и adversarial fixtures

Добавить geometry «шахматка» с плохим face merging, много tiny transparent quads, редкие materials, dense block entities, высокочастотные light changes, rapidly changing inventory, reload во время teleport, необычные skins/fonts, минимум и максимум packet sizes. Пустой uniform terrain недостаточен для проверки memory и meshing.

Race tests: disconnect между decode и apply, remesh после dimension change, resource eviction перед GPU completion, controller expiry во время click, два одинаковых action IDs, WASM disable между host import и result, screenshot при resize. Использовать deterministic scheduler seams там, где это нужно, а не таймеры «на удачу».

## 24. LONG_TERM_PLAN: порядок реализации

**Правило:** каждый этап поставляет реальную проверяемую часть клиента. Native renderer, MCP и asset completeness развиваются вместе; performance не откладывается до конца. До реализации обновить проверенные исходные данные, а не считать этот документ вечной копией API.

### C00 — контракт, provenance и измерительный стенд

Зафиксировать target/versions, licensing boundary, доступ собственного auth приложения, source asset inventory, gap analysis `mc-protocol` и Loader, shared MCP fixtures. Выбрать и закрепить stack; написать минимальный compatibility matrix и perf scenario descriptors. Измерить hardware/capabilities. Подготовить authored fixture pack и независимый vanilla oracle path.

**Выход:** кодовые spikes отвечают на auth/asset/render/MCP feasibility; чёткий список внешних blockers; нет runtime skeleton из десятков пустых crates. Не считать недоступность auth API причиной остановить независимый renderer/MCP fixture development.

### C01 — реальное окно, render loop и MCP до игровой логики

Window lifecycle, device/capability selection, inputs, basic render scene собственного fixture, UI semantics, screenshot, metrics, instance discovery, localhost/stdio MCP и controller safety. Schema/version adapters тестируются отдельно.

**Приёмка:** реальный GPU кадр, resize/focus, input/cancel, authenticated tool discovery, capture. Headless-only не закрывает C01.

### C02 — настоящий vanilla session и первый gameplay loop

Status/login/config/Play, chunk decode/light, камеры, player prediction, базовые модели/texture resources, hotbar/inventory, break/place и звук. Подключение к официальному target server и Solaris. Обычный игровой survival сценарий: подойти, добыть блок, собрать, использовать, сохранить session correctness.

**Приёмка:** ordinary inputs через MCP; server-observed inventory; actual frame/audio; никаких тайных `give/teleport` для закрытия пользовательского пути. Любой fixture setup обозначен отдельно.

### C03 — масштаб terrain без потери картинки

Bounded streaming, incremental meshing, model baker coverage, culling/instancing baseline, GPU arenas, resource reload и репрезентативная прозрачность. CPU/GPU/frame counters. Первые P01/P02 A/B и correctness reference renderer checks.

**Приёмка:** не только ровный мир: пещеры, растительность, вода, города, edit bursts, teleport и high-res pack memory failures.

### C04 — entities, все взаимодействия, containers и UI

Последовательно закрыть full matrix видов entities/block entities, poses/equipment, vehicles, combat presentation, all inventory modes/screens, effects, chat/text/accessibility, audio variants. Пакетные families и auto-enumeration не заменяют ручную проверку сложных исключений.

**Приёмка:** все обязательные функции target имеют coverage; placeholders и unsupported явно блокируют соответствующую completeness строку. Толпы и P03 уже измеряются.

### C05 — Solaris native compatibility

Утвердить native platform/capabilities, перенести Loader codec semantics на Rust, shared test fixtures, exact permissions/activation, generic namespaced UI, custom content mapping и server disconnect cleanup. Не создавать feature shortcuts только для settlements.

**Приёмка:** реальные standard addon scenarios, construction preview, warehouse UI, army commands через UI, звук и dynamic content на том же server build, что Java+Loader oracle.

### C06 — полноценный client-WASM host

Отдельный WIT, permissions, event subscriptions, bounded host commands, resource quotas/lifecycle, error isolation, optional client storage. Выпустить representative open-source client component; server-side modules остаются серверными.

**Приёмка:** полезное расширение UI/animation + adversarial components; нет render-thread stalls и скрытых полномочий. Client-WASM невозможно включить только по требованию вредоносного сервера без local consent.

### C07 — GPU-driven fast path и большой бой

Indirect submission/compaction, compact geometry/instances, shared skinning и relevant optimizations по профилю. Full P01–P05, correctness under same image settings. Затем optional mesh-shader/native backend spike.

**Приёмка:** измеренные CPU/GPU/VRAM улучшения, одинаковые scenes, отсутствие visual regression. Не принимается fast path, который работает только на одной видеокарте и ломает базовый renderer.

### C08 — singleplayer, services и desktop completeness

Управление local Solaris worlds, backup/import, pause/LAN, supported official services, полнота screenshots/audio/settings/input на Windows/Linux/macOS. Закрыть внешние API blockers либо сохранить соответствующий статус и не объявлять сервисную полноту.

**Приёмка:** Java-client-compatible и Solaris-native claims основаны на матрице; native singleplayer claims ограничены фактической server parity, без скрытой JVM.

### C09 — agents, reliability, security, distribution

Полный MCP catalog и guides, migration существующих harness consumers, race/fuzz/adversarial/soak, evidence redaction, signed reproducible distribution, clean install, upgrade/rollback. Проверить минимум две независимые MCP host implementations, выбранные пользователем/проектом, а не только свой тестовый JSON sender.

**Приёмка:** независимый агент поднимает test profile по документации, проходит gameplay+Solaris UI, диагностирует преднамеренно сломанный shader/resource/state и сохраняет доказательства. Full workload повторяется на approved hardware.

### C10 — конечная приёмка и дальнейшие эксперименты

Полная gameplay/UI/entity/resource matrix, независимый oracle, публичный список поддержанных versions/platforms/services, perf raw reports и пользовательская сессия без debug hooks. Optional LOD/RTX-like enhancements/shaderpack adapters рассматриваются после того, как имеют собственный quality contract; не требуют ухудшить Parity.

Клиент не объявлять законченным из-за закрытых C00–C03. Отдельные готовые этапы можно выпускать как preview с точной границей функциональности.

## 25. Организация работы Opus и границы изменений

### 25.1. Первое задание

1. Прочитать актуальные `AGENTS.md`/cursors в связанных репозиториях и сохранить чужие изменения. Исследовать только нужные области, не собирать новый mega-bundle всего сервера.
2. Создать новый client workspace и документы из вводной инструкции. Сохранить требования этого ТЗ с IDs и source references; не превратить их в десять расплывчатых TODO.
3. Выполнить C00: target lock, auth/asset risks, backend selection, protocol/Loader/MCP gap matrix, own fixture и perf workload definitions.
4. Реализовать C01–C02 через минимальные полноценные вертикальные slices. Не начинать с всех будущих crates/API abstractions. Независимые API blockers не заменять обходами; остальные работы продолжаются.
5. На содержательных границах запускать соответствующие tests и независимый read-only review. Не делать commit/push/publish без явной авторизации владельца.

### 25.2. Source-of-truth и upstream work

Новые общие server/client protocol/data contracts принадлежат owning repo и версионируются. Client package с pinned dependency должен собираться воспроизводимо; production не зависит от случайного соседнего dirty checkout. Dev path override допускается только явно.

Не менять server tick budget, scene population, permission rules или terrain semantics ради клиента. Новый native-platform handshake и client-content capacities — реальные upstream changes с собственными tests и compatibility evidence. Не обходить текущие validators через ложный `fabric` identity.

Java Loader не удалять: он остаётся поддержкой официального клиента и независимым oracle. Новый клиент не требует, чтобы сервер отказался от vanilla players. Deprecation любой старой MCP поверхности только после migration всех её реальных consumers.

### 25.3. Стандарты кода

Явные структуры, небольшие state transitions, bounded queues, один владелец каждой mutable области. Comments объясняют invariants/причины, а не пересказывают синтаксис. Profiling marks связывают requests/jobs/frames без global logging на hot loop.

Никакого оптимизирующего generic framework без работающего consumer. Не переносить половину ECS в GPU ради эффектной архитектуры. Не переписывать надёжную библиотеку crypto/image/font/MCP без конкретного нужного свойства. Не заявлять «без allocations» по одному happy path; измерять реальные scenes/reload/errors.

Release должен быть работоспособен без debug-MCP и без доступа к LLM сервису. Отключённая automation не устанавливает middleware на каждый entity/frame.

## 26. Решения по сложным компромиссам

### 26.1. Direct Vulkan против wgpu

Выбран portable wgpu renderer. Прямой Vulkan не даёт автоматического прироста, но создаёт отдельную цену correctness/platform maintenance. Его рассмотрение допускается, когда профилированное ограничение нельзя решить в выбранном backend и prototype выигрывает end-to-end. Отдельный Vulkan path не даёт разрешение бросить Metal/DX12 или держать две копии gameplay.

### 26.2. GPU meshing и ray tracing

GPU meshing может убрать CPU-bound rebuild, но добавляет передачи block/light data, synchronization, memory и сложность general models. Запустить только после измерения CPU bottleneck и сравнить полную стоимость. Не путать shader-generated кубы с полным vanilla block-model pipeline.

Ray tracing/ray marching всего мира, sparse voxel octree renderer и прочие альтернативы — исследования, не обязательный baseline. Их надо проверить на изменяемой геометрии, translucency, tiny models и слабых GPU. Дальний surface-LOD допускается в отдельном профиле; он не заменяет близкую геометрию и взаимодействия.

### 26.3. Полная совместимость картинки и радикальные ускорения

Большую часть CPU-side overhead можно пытаться убрать без изменения логики; точную достижимую кратность покажут benchmarks. Когда требуется visual approximation, она именуется отдельной настройкой и получает собственную приёмку. Ни один режим не меняет серверную экономику, урон и authoritative inventory.

### 26.4. Vanilla asset packs и собственный shader pipeline

Поддержка textures/models не означает автоматическую поддержку custom shader source. Совместимость фиксируется на уровне capabilities и tested pack features. Будущая shader translation должна закрыть frontend language, uniforms/resources/pass semantics, precision и GPU safety. До этого unsupported pack явно отказывается/работает в обозначенном partial режиме, не молча теряет эффекты под полным claim.

### 26.5. Ширина MCP

Нужна широкая функциональная поверхность, но не тысячи команд и не arbitrary eval. Типизированные действия и bounded queries покрывают primitives; сценарии составляют их в workflows. Tool catalog стабилен, guides/schema generated, большие данные вынесены в resources. Новая механика сначала получает нормальный action/UI contract, затем MCP adapter.

### 26.6. Full client против полной игры

Внешний сервер уже владеет большинством game simulation. Поэтому полноценный multiplayer client не обязан запускать весь AI. Но native singleplayer зависит от полноты local Solaris. Отдельные milestones защищают от обещания полной игры на основании корректного рендеринга и networking.

## 27. Проверенная исходная точка и источники

### 27.1. Локальные исходники, просмотренные через codexpro

Рабочие деревья просмотрены 2026-10-01. Они содержали незакоммиченные изменения. Ниже hashes отдельных прочитанных файлов, не fingerprint всего проекта. Код репозиториев в этой работе не менялся; новые build/performance gates не запускались.

- **[R01]** `solaris-loader/AGENTS.md:33–42`: wire 3/schema 2 и распределение ответственности. SHA-256 `3066fb89debcf14a88a93afb879bd14108dbfde4f338f87808cae3e1e971839b`.
- **[R02]** `solaris-loader/loader-core/src/main/java/dev/solaris/loader/LoaderPlatform.java:5–11`: enum Fabric/NeoForge/Forge. SHA-256 `57d977f8f644d2989047162a1b3d6ddef597bad90cee561b14e36de37f5779f6`.
- **[R03]** `solaris-loader/README.md:280–430`: current automation launch/catalog/waits, namespaced `view_key`, loopback/auth/producer events. SHA-256 `e53df3a8092eb83979c1a41d31f7ca121a2f8fec3b5da3601b0cae938e34a539`.
- **[R04]** `solaris-loader/bridge-core/src/main/java/dev/solaris/agent/mcp/ClientMcpTools.java:17–200`: реальные typed tools, waits, entity fences, normal input semantics. SHA-256 `4b4315d67c1cffdcf21ddf7a14a5d0f608dcdb3fe362f2f2051da11d4ffb973c`. Для полного catalog C00 должен прочитать оставшуюся часть, а не ограничиться этим диапазоном.
- **[R05]** `solaris-loader/bridge-core/src/main/java/dev/solaris/agent/mcp/McpHttpServer.java:35–39,233–266`: targeted search подтвердил revisions 2025-11-25/2025-06-18. Полный transport audit здесь не проводился.
- **[R06]** `solaris/crates/mc-protocol/Cargo.toml:14–21`: dependencies `mc-data/mc-domain/mc-nbt/bytes/flate2/thiserror/uuid`. SHA-256 `d6b9c32036aae5e25462749c278ecd87c877c5b757628c4a7d97fc938a8c69c4`.

Наличие готового производственного Rust-клиента в этих данных не установлено. Изученные репозитории дают server contracts, Java Loader и test automation. Настоящее ТЗ проектирует новый продукт, а не описывает найденную реализацию.

### 27.2. Внешние первичные источники

Прочитаны 2026-10-01. Это reference material, не доказательство соответствия будущего кода. Перед pin зависимостей и release снова проверить актуальность.

- **[S01]** Mojang: Java Edition 26.1.2 release — https://www.minecraft.net/en-us/article/minecraft-java-edition-26-1-2
- **[S02]** Minecraft EULA — https://www.minecraft.net/en-us/eula
- **[S03]** Minecraft Usage Guidelines — https://www.minecraft.net/en-us/usage-guidelines
- **[S04]** Microsoft identity platform: application scenarios/auth flows — https://learn.microsoft.com/en-us/entra/identity-platform/authentication-flows-app-scenarios
- **[S05]** wgpu project — https://wgpu.rs/
- **[S06]** winit documentation — https://docs.rs/winit/latest/winit/
- **[S07]** wgpu native feature reference — https://wgpu.rs/doc/wgpu/struct.FeaturesWGPU.html
- **[S08]** Sodium upstream/license — https://github.com/CaffeineMC/sodium ; https://raw.githubusercontent.com/CaffeineMC/sodium/dev/LICENSE.md
- **[S09]** Nvidium upstream — https://github.com/MCRcortex/nvidium
- **[S10]** Khronos: GPU Rendering and Multi-Draw Indirect — https://docs.vulkan.org/samples/latest/samples/performance/multi_draw_indirect/README.html
- **[S11]** Wasmtime Config: fuel/epochs and blocking host calls — https://docs.wasmtime.dev/api/wasmtime/struct.Config.html
- **[S12]** Wasmtime security — https://docs.wasmtime.dev/security.html
- **[S13]** Wasmtime Module/deserialize safety — https://docs.rs/wasmtime/latest/wasmtime/struct.Module.html
- **[S14]** MCP current specification, resolved to 2026-07-28 — https://modelcontextprotocol.io/specification/latest
- **[S15]** MCP 2026-07-28 transports — https://modelcontextprotocol.io/specification/2026-07-28/basic/transports
- **[S16]** MCP 2026-07-28 tools — https://modelcontextprotocol.io/specification/2026-07-28/server/tools
- **[S17]** MCP 2026-07-28 authorization — https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization
- **[S18]** MCP 2026-07-28 versioning — https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning
- **[S19]** Official MCP Rust SDK — https://github.com/modelcontextprotocol/rust-sdk
- **[S20]** MCP 2025-11-25 transports — https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- **[S21]** MCP resources — https://modelcontextprotocol.io/specification/2026-07-28/server/resources
- **[S22]** Prism Launcher authored asset-index/checksum download implementation, reference only, не код для копирования без GPL review — https://raw.githubusercontent.com/PrismLauncher/PrismLauncher/develop/launcher/minecraft/update/AssetUpdateTask.cpp
- **[S23]** Prism Launcher own-client-ID/distribution policy, reference only — https://github.com/PrismLauncher/PrismLauncher

Свежее скачивание официального version manifest из рабочего контейнера не удалось из-за сетевого/DNS ограничения. Поэтому этот документ не содержит выдуманных SHA для Mojang JAR/assets, не утверждает успешную проверку entitlement и оставляет получение настоящих pinned digests этапу C00. Механизм asset-index/checksum сверялся с первичной реализацией [S22]; права и правила загрузки требуют отдельной проверки [S02–S04].

## 28. Условия конечного результата

Продукт принят для явно указанного уровня совместимости, когда пользователь может законно получить ресурсы, запустить собственный Rust-клиент, выполнить полный заявленный gameplay/UI цикл на независимом vanilla и Solaris сервере, видеть корректную картинку/слышать звук, использовать согласованные Solaris/client-WASM аддоны, а агент способен воспроизвести и проверить тот же путь через безопасный MCP.

Производительность подтверждается сохранёнными full-scene измерениями при одинаковых настройках. Полнота подтверждается coverage matrix и реальными сценариями. Безопасность подтверждается negative/adversarial tests и проверенной поставкой. Ни одно из этих трёх свойств не заменяет другое.

**Ключевой принцип:** убрать лишнюю работу и лишние посредники, сохранив действительное поведение клиента, наблюдаемую причинность и проверяемый результат.


## Exact source-line traceability ledger

All refs below are to the authority path above, not generated-document line numbers. Blank source lines are formatting only and are retained in the baseline.

| Stable ID | Original source line |
|---|---|
| <a id="src-0001"></a>`SRC-0001` | `1` |
| <a id="src-0003"></a>`SRC-0003` | `3` |
| <a id="src-0004"></a>`SRC-0004` | `4` |
| <a id="src-0005"></a>`SRC-0005` | `5` |
| <a id="src-0006"></a>`SRC-0006` | `6` |
| <a id="src-0008"></a>`SRC-0008` | `8` |
| <a id="src-0010"></a>`SRC-0010` | `10` |
| <a id="src-0012"></a>`SRC-0012` | `12` |
| <a id="src-0014"></a>`SRC-0014` | `14` |
| <a id="src-0016"></a>`SRC-0016` | `16` |
| <a id="src-0018"></a>`SRC-0018` | `18` |
| <a id="src-0020"></a>`SRC-0020` | `20` |
| <a id="src-0022"></a>`SRC-0022` | `22` |
| <a id="src-0024"></a>`SRC-0024` | `24` |
| <a id="src-0026"></a>`SRC-0026` | `26` |
| <a id="src-0028"></a>`SRC-0028` | `28` |
| <a id="src-0030"></a>`SRC-0030` | `30` |
| <a id="src-0032"></a>`SRC-0032` | `32` |
| <a id="src-0034"></a>`SRC-0034` | `34` |
| <a id="src-0036"></a>`SRC-0036` | `36` |
| <a id="src-0037"></a>`SRC-0037` | `37` |
| <a id="src-0038"></a>`SRC-0038` | `38` |
| <a id="src-0039"></a>`SRC-0039` | `39` |
| <a id="src-0040"></a>`SRC-0040` | `40` |
| <a id="src-0041"></a>`SRC-0041` | `41` |
| <a id="src-0042"></a>`SRC-0042` | `42` |
| <a id="src-0043"></a>`SRC-0043` | `43` |
| <a id="src-0045"></a>`SRC-0045` | `45` |
| <a id="src-0047"></a>`SRC-0047` | `47` |
| <a id="src-0049"></a>`SRC-0049` | `49` |
| <a id="src-0051"></a>`SRC-0051` | `51` |
| <a id="src-0053"></a>`SRC-0053` | `53` |
| <a id="src-0055"></a>`SRC-0055` | `55` |
| <a id="src-0056"></a>`SRC-0056` | `56` |
| <a id="src-0057"></a>`SRC-0057` | `57` |
| <a id="src-0058"></a>`SRC-0058` | `58` |
| <a id="src-0059"></a>`SRC-0059` | `59` |
| <a id="src-0060"></a>`SRC-0060` | `60` |
| <a id="src-0061"></a>`SRC-0061` | `61` |
| <a id="src-0062"></a>`SRC-0062` | `62` |
| <a id="src-0063"></a>`SRC-0063` | `63` |
| <a id="src-0064"></a>`SRC-0064` | `64` |
| <a id="src-0065"></a>`SRC-0065` | `65` |
| <a id="src-0066"></a>`SRC-0066` | `66` |
| <a id="src-0068"></a>`SRC-0068` | `68` |
| <a id="src-0070"></a>`SRC-0070` | `70` |
| <a id="src-0072"></a>`SRC-0072` | `72` |
| <a id="src-0074"></a>`SRC-0074` | `74` |
| <a id="src-0076"></a>`SRC-0076` | `76` |
| <a id="src-0078"></a>`SRC-0078` | `78` |
| <a id="src-0080"></a>`SRC-0080` | `80` |
| <a id="src-0082"></a>`SRC-0082` | `82` |
| <a id="src-0084"></a>`SRC-0084` | `84` |
| <a id="src-0086"></a>`SRC-0086` | `86` |
| <a id="src-0088"></a>`SRC-0088` | `88` |
| <a id="src-0090"></a>`SRC-0090` | `90` |
| <a id="src-0092"></a>`SRC-0092` | `92` |
| <a id="src-0094"></a>`SRC-0094` | `94` |
| <a id="src-0095"></a>`SRC-0095` | `95` |
| <a id="src-0096"></a>`SRC-0096` | `96` |
| <a id="src-0097"></a>`SRC-0097` | `97` |
| <a id="src-0098"></a>`SRC-0098` | `98` |
| <a id="src-0099"></a>`SRC-0099` | `99` |
| <a id="src-0100"></a>`SRC-0100` | `100` |
| <a id="src-0101"></a>`SRC-0101` | `101` |
| <a id="src-0102"></a>`SRC-0102` | `102` |
| <a id="src-0104"></a>`SRC-0104` | `104` |
| <a id="src-0106"></a>`SRC-0106` | `106` |
| <a id="src-0108"></a>`SRC-0108` | `108` |
| <a id="src-0110"></a>`SRC-0110` | `110` |
| <a id="src-0112"></a>`SRC-0112` | `112` |
| <a id="src-0114"></a>`SRC-0114` | `114` |
| <a id="src-0116"></a>`SRC-0116` | `116` |
| <a id="src-0118"></a>`SRC-0118` | `118` |
| <a id="src-0120"></a>`SRC-0120` | `120` |
| <a id="src-0122"></a>`SRC-0122` | `122` |
| <a id="src-0124"></a>`SRC-0124` | `124` |
| <a id="src-0125"></a>`SRC-0125` | `125` |
| <a id="src-0126"></a>`SRC-0126` | `126` |
| <a id="src-0127"></a>`SRC-0127` | `127` |
| <a id="src-0128"></a>`SRC-0128` | `128` |
| <a id="src-0129"></a>`SRC-0129` | `129` |
| <a id="src-0130"></a>`SRC-0130` | `130` |
| <a id="src-0131"></a>`SRC-0131` | `131` |
| <a id="src-0132"></a>`SRC-0132` | `132` |
| <a id="src-0133"></a>`SRC-0133` | `133` |
| <a id="src-0134"></a>`SRC-0134` | `134` |
| <a id="src-0135"></a>`SRC-0135` | `135` |
| <a id="src-0136"></a>`SRC-0136` | `136` |
| <a id="src-0137"></a>`SRC-0137` | `137` |
| <a id="src-0138"></a>`SRC-0138` | `138` |
| <a id="src-0139"></a>`SRC-0139` | `139` |
| <a id="src-0141"></a>`SRC-0141` | `141` |
| <a id="src-0143"></a>`SRC-0143` | `143` |
| <a id="src-0145"></a>`SRC-0145` | `145` |
| <a id="src-0147"></a>`SRC-0147` | `147` |
| <a id="src-0149"></a>`SRC-0149` | `149` |
| <a id="src-0151"></a>`SRC-0151` | `151` |
| <a id="src-0152"></a>`SRC-0152` | `152` |
| <a id="src-0153"></a>`SRC-0153` | `153` |
| <a id="src-0154"></a>`SRC-0154` | `154` |
| <a id="src-0155"></a>`SRC-0155` | `155` |
| <a id="src-0156"></a>`SRC-0156` | `156` |
| <a id="src-0157"></a>`SRC-0157` | `157` |
| <a id="src-0158"></a>`SRC-0158` | `158` |
| <a id="src-0160"></a>`SRC-0160` | `160` |
| <a id="src-0162"></a>`SRC-0162` | `162` |
| <a id="src-0164"></a>`SRC-0164` | `164` |
| <a id="src-0166"></a>`SRC-0166` | `166` |
| <a id="src-0167"></a>`SRC-0167` | `167` |
| <a id="src-0168"></a>`SRC-0168` | `168` |
| <a id="src-0169"></a>`SRC-0169` | `169` |
| <a id="src-0171"></a>`SRC-0171` | `171` |
| <a id="src-0172"></a>`SRC-0172` | `172` |
| <a id="src-0173"></a>`SRC-0173` | `173` |
| <a id="src-0174"></a>`SRC-0174` | `174` |
| <a id="src-0175"></a>`SRC-0175` | `175` |
| <a id="src-0176"></a>`SRC-0176` | `176` |
| <a id="src-0178"></a>`SRC-0178` | `178` |
| <a id="src-0179"></a>`SRC-0179` | `179` |
| <a id="src-0181"></a>`SRC-0181` | `181` |
| <a id="src-0183"></a>`SRC-0183` | `183` |
| <a id="src-0185"></a>`SRC-0185` | `185` |
| <a id="src-0187"></a>`SRC-0187` | `187` |
| <a id="src-0189"></a>`SRC-0189` | `189` |
| <a id="src-0191"></a>`SRC-0191` | `191` |
| <a id="src-0193"></a>`SRC-0193` | `193` |
| <a id="src-0195"></a>`SRC-0195` | `195` |
| <a id="src-0197"></a>`SRC-0197` | `197` |
| <a id="src-0198"></a>`SRC-0198` | `198` |
| <a id="src-0199"></a>`SRC-0199` | `199` |
| <a id="src-0200"></a>`SRC-0200` | `200` |
| <a id="src-0201"></a>`SRC-0201` | `201` |
| <a id="src-0202"></a>`SRC-0202` | `202` |
| <a id="src-0203"></a>`SRC-0203` | `203` |
| <a id="src-0204"></a>`SRC-0204` | `204` |
| <a id="src-0205"></a>`SRC-0205` | `205` |
| <a id="src-0207"></a>`SRC-0207` | `207` |
| <a id="src-0209"></a>`SRC-0209` | `209` |
| <a id="src-0211"></a>`SRC-0211` | `211` |
| <a id="src-0213"></a>`SRC-0213` | `213` |
| <a id="src-0215"></a>`SRC-0215` | `215` |
| <a id="src-0217"></a>`SRC-0217` | `217` |
| <a id="src-0219"></a>`SRC-0219` | `219` |
| <a id="src-0221"></a>`SRC-0221` | `221` |
| <a id="src-0223"></a>`SRC-0223` | `223` |
| <a id="src-0225"></a>`SRC-0225` | `225` |
| <a id="src-0227"></a>`SRC-0227` | `227` |
| <a id="src-0229"></a>`SRC-0229` | `229` |
| <a id="src-0231"></a>`SRC-0231` | `231` |
| <a id="src-0233"></a>`SRC-0233` | `233` |
| <a id="src-0235"></a>`SRC-0235` | `235` |
| <a id="src-0237"></a>`SRC-0237` | `237` |
| <a id="src-0239"></a>`SRC-0239` | `239` |
| <a id="src-0241"></a>`SRC-0241` | `241` |
| <a id="src-0243"></a>`SRC-0243` | `243` |
| <a id="src-0245"></a>`SRC-0245` | `245` |
| <a id="src-0247"></a>`SRC-0247` | `247` |
| <a id="src-0249"></a>`SRC-0249` | `249` |
| <a id="src-0251"></a>`SRC-0251` | `251` |
| <a id="src-0253"></a>`SRC-0253` | `253` |
| <a id="src-0255"></a>`SRC-0255` | `255` |
| <a id="src-0257"></a>`SRC-0257` | `257` |
| <a id="src-0259"></a>`SRC-0259` | `259` |
| <a id="src-0261"></a>`SRC-0261` | `261` |
| <a id="src-0263"></a>`SRC-0263` | `263` |
| <a id="src-0265"></a>`SRC-0265` | `265` |
| <a id="src-0267"></a>`SRC-0267` | `267` |
| <a id="src-0269"></a>`SRC-0269` | `269` |
| <a id="src-0271"></a>`SRC-0271` | `271` |
| <a id="src-0273"></a>`SRC-0273` | `273` |
| <a id="src-0275"></a>`SRC-0275` | `275` |
| <a id="src-0277"></a>`SRC-0277` | `277` |
| <a id="src-0279"></a>`SRC-0279` | `279` |
| <a id="src-0281"></a>`SRC-0281` | `281` |
| <a id="src-0283"></a>`SRC-0283` | `283` |
| <a id="src-0284"></a>`SRC-0284` | `284` |
| <a id="src-0285"></a>`SRC-0285` | `285` |
| <a id="src-0287"></a>`SRC-0287` | `287` |
| <a id="src-0289"></a>`SRC-0289` | `289` |
| <a id="src-0291"></a>`SRC-0291` | `291` |
| <a id="src-0293"></a>`SRC-0293` | `293` |
| <a id="src-0295"></a>`SRC-0295` | `295` |
| <a id="src-0296"></a>`SRC-0296` | `296` |
| <a id="src-0297"></a>`SRC-0297` | `297` |
| <a id="src-0298"></a>`SRC-0298` | `298` |
| <a id="src-0299"></a>`SRC-0299` | `299` |
| <a id="src-0300"></a>`SRC-0300` | `300` |
| <a id="src-0301"></a>`SRC-0301` | `301` |
| <a id="src-0302"></a>`SRC-0302` | `302` |
| <a id="src-0303"></a>`SRC-0303` | `303` |
| <a id="src-0304"></a>`SRC-0304` | `304` |
| <a id="src-0306"></a>`SRC-0306` | `306` |
| <a id="src-0308"></a>`SRC-0308` | `308` |
| <a id="src-0310"></a>`SRC-0310` | `310` |
| <a id="src-0312"></a>`SRC-0312` | `312` |
| <a id="src-0314"></a>`SRC-0314` | `314` |
| <a id="src-0316"></a>`SRC-0316` | `316` |
| <a id="src-0318"></a>`SRC-0318` | `318` |
| <a id="src-0320"></a>`SRC-0320` | `320` |
| <a id="src-0322"></a>`SRC-0322` | `322` |
| <a id="src-0324"></a>`SRC-0324` | `324` |
| <a id="src-0326"></a>`SRC-0326` | `326` |
| <a id="src-0328"></a>`SRC-0328` | `328` |
| <a id="src-0330"></a>`SRC-0330` | `330` |
| <a id="src-0332"></a>`SRC-0332` | `332` |
| <a id="src-0334"></a>`SRC-0334` | `334` |
| <a id="src-0336"></a>`SRC-0336` | `336` |
| <a id="src-0338"></a>`SRC-0338` | `338` |
| <a id="src-0340"></a>`SRC-0340` | `340` |
| <a id="src-0342"></a>`SRC-0342` | `342` |
| <a id="src-0344"></a>`SRC-0344` | `344` |
| <a id="src-0346"></a>`SRC-0346` | `346` |
| <a id="src-0348"></a>`SRC-0348` | `348` |
| <a id="src-0350"></a>`SRC-0350` | `350` |
| <a id="src-0352"></a>`SRC-0352` | `352` |
| <a id="src-0354"></a>`SRC-0354` | `354` |
| <a id="src-0356"></a>`SRC-0356` | `356` |
| <a id="src-0358"></a>`SRC-0358` | `358` |
| <a id="src-0360"></a>`SRC-0360` | `360` |
| <a id="src-0362"></a>`SRC-0362` | `362` |
| <a id="src-0364"></a>`SRC-0364` | `364` |
| <a id="src-0366"></a>`SRC-0366` | `366` |
| <a id="src-0368"></a>`SRC-0368` | `368` |
| <a id="src-0370"></a>`SRC-0370` | `370` |
| <a id="src-0372"></a>`SRC-0372` | `372` |
| <a id="src-0374"></a>`SRC-0374` | `374` |
| <a id="src-0376"></a>`SRC-0376` | `376` |
| <a id="src-0378"></a>`SRC-0378` | `378` |
| <a id="src-0380"></a>`SRC-0380` | `380` |
| <a id="src-0382"></a>`SRC-0382` | `382` |
| <a id="src-0384"></a>`SRC-0384` | `384` |
| <a id="src-0386"></a>`SRC-0386` | `386` |
| <a id="src-0388"></a>`SRC-0388` | `388` |
| <a id="src-0390"></a>`SRC-0390` | `390` |
| <a id="src-0392"></a>`SRC-0392` | `392` |
| <a id="src-0394"></a>`SRC-0394` | `394` |
| <a id="src-0396"></a>`SRC-0396` | `396` |
| <a id="src-0398"></a>`SRC-0398` | `398` |
| <a id="src-0400"></a>`SRC-0400` | `400` |
| <a id="src-0402"></a>`SRC-0402` | `402` |
| <a id="src-0404"></a>`SRC-0404` | `404` |
| <a id="src-0406"></a>`SRC-0406` | `406` |
| <a id="src-0408"></a>`SRC-0408` | `408` |
| <a id="src-0410"></a>`SRC-0410` | `410` |
| <a id="src-0412"></a>`SRC-0412` | `412` |
| <a id="src-0414"></a>`SRC-0414` | `414` |
| <a id="src-0416"></a>`SRC-0416` | `416` |
| <a id="src-0418"></a>`SRC-0418` | `418` |
| <a id="src-0420"></a>`SRC-0420` | `420` |
| <a id="src-0422"></a>`SRC-0422` | `422` |
| <a id="src-0424"></a>`SRC-0424` | `424` |
| <a id="src-0426"></a>`SRC-0426` | `426` |
| <a id="src-0428"></a>`SRC-0428` | `428` |
| <a id="src-0430"></a>`SRC-0430` | `430` |
| <a id="src-0432"></a>`SRC-0432` | `432` |
| <a id="src-0434"></a>`SRC-0434` | `434` |
| <a id="src-0436"></a>`SRC-0436` | `436` |
| <a id="src-0437"></a>`SRC-0437` | `437` |
| <a id="src-0438"></a>`SRC-0438` | `438` |
| <a id="src-0439"></a>`SRC-0439` | `439` |
| <a id="src-0440"></a>`SRC-0440` | `440` |
| <a id="src-0441"></a>`SRC-0441` | `441` |
| <a id="src-0442"></a>`SRC-0442` | `442` |
| <a id="src-0443"></a>`SRC-0443` | `443` |
| <a id="src-0444"></a>`SRC-0444` | `444` |
| <a id="src-0445"></a>`SRC-0445` | `445` |
| <a id="src-0446"></a>`SRC-0446` | `446` |
| <a id="src-0448"></a>`SRC-0448` | `448` |
| <a id="src-0450"></a>`SRC-0450` | `450` |
| <a id="src-0452"></a>`SRC-0452` | `452` |
| <a id="src-0454"></a>`SRC-0454` | `454` |
| <a id="src-0456"></a>`SRC-0456` | `456` |
| <a id="src-0458"></a>`SRC-0458` | `458` |
| <a id="src-0460"></a>`SRC-0460` | `460` |
| <a id="src-0462"></a>`SRC-0462` | `462` |
| <a id="src-0464"></a>`SRC-0464` | `464` |
| <a id="src-0466"></a>`SRC-0466` | `466` |
| <a id="src-0468"></a>`SRC-0468` | `468` |
| <a id="src-0470"></a>`SRC-0470` | `470` |
| <a id="src-0472"></a>`SRC-0472` | `472` |
| <a id="src-0474"></a>`SRC-0474` | `474` |
| <a id="src-0476"></a>`SRC-0476` | `476` |
| <a id="src-0478"></a>`SRC-0478` | `478` |
| <a id="src-0480"></a>`SRC-0480` | `480` |
| <a id="src-0482"></a>`SRC-0482` | `482` |
| <a id="src-0484"></a>`SRC-0484` | `484` |
| <a id="src-0486"></a>`SRC-0486` | `486` |
| <a id="src-0488"></a>`SRC-0488` | `488` |
| <a id="src-0490"></a>`SRC-0490` | `490` |
| <a id="src-0492"></a>`SRC-0492` | `492` |
| <a id="src-0494"></a>`SRC-0494` | `494` |
| <a id="src-0496"></a>`SRC-0496` | `496` |
| <a id="src-0498"></a>`SRC-0498` | `498` |
| <a id="src-0500"></a>`SRC-0500` | `500` |
| <a id="src-0502"></a>`SRC-0502` | `502` |
| <a id="src-0504"></a>`SRC-0504` | `504` |
| <a id="src-0506"></a>`SRC-0506` | `506` |
| <a id="src-0508"></a>`SRC-0508` | `508` |
| <a id="src-0510"></a>`SRC-0510` | `510` |
| <a id="src-0512"></a>`SRC-0512` | `512` |
| <a id="src-0514"></a>`SRC-0514` | `514` |
| <a id="src-0516"></a>`SRC-0516` | `516` |
| <a id="src-0518"></a>`SRC-0518` | `518` |
| <a id="src-0520"></a>`SRC-0520` | `520` |
| <a id="src-0522"></a>`SRC-0522` | `522` |
| <a id="src-0524"></a>`SRC-0524` | `524` |
| <a id="src-0526"></a>`SRC-0526` | `526` |
| <a id="src-0528"></a>`SRC-0528` | `528` |
| <a id="src-0530"></a>`SRC-0530` | `530` |
| <a id="src-0532"></a>`SRC-0532` | `532` |
| <a id="src-0534"></a>`SRC-0534` | `534` |
| <a id="src-0536"></a>`SRC-0536` | `536` |
| <a id="src-0538"></a>`SRC-0538` | `538` |
| <a id="src-0540"></a>`SRC-0540` | `540` |
| <a id="src-0542"></a>`SRC-0542` | `542` |
| <a id="src-0544"></a>`SRC-0544` | `544` |
| <a id="src-0546"></a>`SRC-0546` | `546` |
| <a id="src-0548"></a>`SRC-0548` | `548` |
| <a id="src-0550"></a>`SRC-0550` | `550` |
| <a id="src-0552"></a>`SRC-0552` | `552` |
| <a id="src-0553"></a>`SRC-0553` | `553` |
| <a id="src-0554"></a>`SRC-0554` | `554` |
| <a id="src-0556"></a>`SRC-0556` | `556` |
| <a id="src-0558"></a>`SRC-0558` | `558` |
| <a id="src-0560"></a>`SRC-0560` | `560` |
| <a id="src-0562"></a>`SRC-0562` | `562` |
| <a id="src-0563"></a>`SRC-0563` | `563` |
| <a id="src-0564"></a>`SRC-0564` | `564` |
| <a id="src-0565"></a>`SRC-0565` | `565` |
| <a id="src-0566"></a>`SRC-0566` | `566` |
| <a id="src-0567"></a>`SRC-0567` | `567` |
| <a id="src-0568"></a>`SRC-0568` | `568` |
| <a id="src-0569"></a>`SRC-0569` | `569` |
| <a id="src-0571"></a>`SRC-0571` | `571` |
| <a id="src-0573"></a>`SRC-0573` | `573` |
| <a id="src-0575"></a>`SRC-0575` | `575` |
| <a id="src-0577"></a>`SRC-0577` | `577` |
| <a id="src-0579"></a>`SRC-0579` | `579` |
| <a id="src-0581"></a>`SRC-0581` | `581` |
| <a id="src-0583"></a>`SRC-0583` | `583` |
| <a id="src-0584"></a>`SRC-0584` | `584` |
| <a id="src-0585"></a>`SRC-0585` | `585` |
| <a id="src-0586"></a>`SRC-0586` | `586` |
| <a id="src-0587"></a>`SRC-0587` | `587` |
| <a id="src-0588"></a>`SRC-0588` | `588` |
| <a id="src-0589"></a>`SRC-0589` | `589` |
| <a id="src-0591"></a>`SRC-0591` | `591` |
| <a id="src-0593"></a>`SRC-0593` | `593` |
| <a id="src-0595"></a>`SRC-0595` | `595` |
| <a id="src-0597"></a>`SRC-0597` | `597` |
| <a id="src-0599"></a>`SRC-0599` | `599` |
| <a id="src-0601"></a>`SRC-0601` | `601` |
| <a id="src-0603"></a>`SRC-0603` | `603` |
| <a id="src-0605"></a>`SRC-0605` | `605` |
| <a id="src-0607"></a>`SRC-0607` | `607` |
| <a id="src-0609"></a>`SRC-0609` | `609` |
| <a id="src-0611"></a>`SRC-0611` | `611` |
| <a id="src-0613"></a>`SRC-0613` | `613` |
| <a id="src-0615"></a>`SRC-0615` | `615` |
| <a id="src-0616"></a>`SRC-0616` | `616` |
| <a id="src-0617"></a>`SRC-0617` | `617` |
| <a id="src-0618"></a>`SRC-0618` | `618` |
| <a id="src-0619"></a>`SRC-0619` | `619` |
| <a id="src-0620"></a>`SRC-0620` | `620` |
| <a id="src-0621"></a>`SRC-0621` | `621` |
| <a id="src-0622"></a>`SRC-0622` | `622` |
| <a id="src-0623"></a>`SRC-0623` | `623` |
| <a id="src-0624"></a>`SRC-0624` | `624` |
| <a id="src-0625"></a>`SRC-0625` | `625` |
| <a id="src-0626"></a>`SRC-0626` | `626` |
| <a id="src-0627"></a>`SRC-0627` | `627` |
| <a id="src-0628"></a>`SRC-0628` | `628` |
| <a id="src-0629"></a>`SRC-0629` | `629` |
| <a id="src-0630"></a>`SRC-0630` | `630` |
| <a id="src-0631"></a>`SRC-0631` | `631` |
| <a id="src-0633"></a>`SRC-0633` | `633` |
| <a id="src-0635"></a>`SRC-0635` | `635` |
| <a id="src-0637"></a>`SRC-0637` | `637` |
| <a id="src-0639"></a>`SRC-0639` | `639` |
| <a id="src-0641"></a>`SRC-0641` | `641` |
| <a id="src-0642"></a>`SRC-0642` | `642` |
| <a id="src-0643"></a>`SRC-0643` | `643` |
| <a id="src-0644"></a>`SRC-0644` | `644` |
| <a id="src-0646"></a>`SRC-0646` | `646` |
| <a id="src-0648"></a>`SRC-0648` | `648` |
| <a id="src-0650"></a>`SRC-0650` | `650` |
| <a id="src-0652"></a>`SRC-0652` | `652` |
| <a id="src-0654"></a>`SRC-0654` | `654` |
| <a id="src-0656"></a>`SRC-0656` | `656` |
| <a id="src-0658"></a>`SRC-0658` | `658` |
| <a id="src-0660"></a>`SRC-0660` | `660` |
| <a id="src-0662"></a>`SRC-0662` | `662` |
| <a id="src-0664"></a>`SRC-0664` | `664` |
| <a id="src-0666"></a>`SRC-0666` | `666` |
| <a id="src-0668"></a>`SRC-0668` | `668` |
| <a id="src-0670"></a>`SRC-0670` | `670` |
| <a id="src-0671"></a>`SRC-0671` | `671` |
| <a id="src-0672"></a>`SRC-0672` | `672` |
| <a id="src-0673"></a>`SRC-0673` | `673` |
| <a id="src-0674"></a>`SRC-0674` | `674` |
| <a id="src-0675"></a>`SRC-0675` | `675` |
| <a id="src-0676"></a>`SRC-0676` | `676` |
| <a id="src-0677"></a>`SRC-0677` | `677` |
| <a id="src-0678"></a>`SRC-0678` | `678` |
| <a id="src-0679"></a>`SRC-0679` | `679` |
| <a id="src-0680"></a>`SRC-0680` | `680` |
| <a id="src-0681"></a>`SRC-0681` | `681` |
| <a id="src-0682"></a>`SRC-0682` | `682` |
| <a id="src-0683"></a>`SRC-0683` | `683` |
| <a id="src-0684"></a>`SRC-0684` | `684` |
| <a id="src-0685"></a>`SRC-0685` | `685` |
| <a id="src-0686"></a>`SRC-0686` | `686` |
| <a id="src-0687"></a>`SRC-0687` | `687` |
| <a id="src-0689"></a>`SRC-0689` | `689` |
| <a id="src-0691"></a>`SRC-0691` | `691` |
| <a id="src-0693"></a>`SRC-0693` | `693` |
| <a id="src-0695"></a>`SRC-0695` | `695` |
| <a id="src-0697"></a>`SRC-0697` | `697` |
| <a id="src-0699"></a>`SRC-0699` | `699` |
| <a id="src-0701"></a>`SRC-0701` | `701` |
| <a id="src-0703"></a>`SRC-0703` | `703` |
| <a id="src-0705"></a>`SRC-0705` | `705` |
| <a id="src-0707"></a>`SRC-0707` | `707` |
| <a id="src-0709"></a>`SRC-0709` | `709` |
| <a id="src-0711"></a>`SRC-0711` | `711` |
| <a id="src-0713"></a>`SRC-0713` | `713` |
| <a id="src-0715"></a>`SRC-0715` | `715` |
| <a id="src-0717"></a>`SRC-0717` | `717` |
| <a id="src-0719"></a>`SRC-0719` | `719` |
| <a id="src-0720"></a>`SRC-0720` | `720` |
| <a id="src-0721"></a>`SRC-0721` | `721` |
| <a id="src-0722"></a>`SRC-0722` | `722` |
| <a id="src-0723"></a>`SRC-0723` | `723` |
| <a id="src-0724"></a>`SRC-0724` | `724` |
| <a id="src-0725"></a>`SRC-0725` | `725` |
| <a id="src-0727"></a>`SRC-0727` | `727` |
| <a id="src-0729"></a>`SRC-0729` | `729` |
| <a id="src-0731"></a>`SRC-0731` | `731` |
| <a id="src-0733"></a>`SRC-0733` | `733` |
| <a id="src-0735"></a>`SRC-0735` | `735` |
| <a id="src-0736"></a>`SRC-0736` | `736` |
| <a id="src-0737"></a>`SRC-0737` | `737` |
| <a id="src-0738"></a>`SRC-0738` | `738` |
| <a id="src-0739"></a>`SRC-0739` | `739` |
| <a id="src-0740"></a>`SRC-0740` | `740` |
| <a id="src-0741"></a>`SRC-0741` | `741` |
| <a id="src-0743"></a>`SRC-0743` | `743` |
| <a id="src-0745"></a>`SRC-0745` | `745` |
| <a id="src-0747"></a>`SRC-0747` | `747` |
| <a id="src-0749"></a>`SRC-0749` | `749` |
| <a id="src-0751"></a>`SRC-0751` | `751` |
| <a id="src-0753"></a>`SRC-0753` | `753` |
| <a id="src-0755"></a>`SRC-0755` | `755` |
| <a id="src-0757"></a>`SRC-0757` | `757` |
| <a id="src-0759"></a>`SRC-0759` | `759` |
| <a id="src-0761"></a>`SRC-0761` | `761` |
| <a id="src-0763"></a>`SRC-0763` | `763` |
| <a id="src-0765"></a>`SRC-0765` | `765` |
| <a id="src-0767"></a>`SRC-0767` | `767` |
| <a id="src-0769"></a>`SRC-0769` | `769` |
| <a id="src-0771"></a>`SRC-0771` | `771` |
| <a id="src-0773"></a>`SRC-0773` | `773` |
| <a id="src-0775"></a>`SRC-0775` | `775` |
| <a id="src-0777"></a>`SRC-0777` | `777` |
| <a id="src-0779"></a>`SRC-0779` | `779` |
| <a id="src-0781"></a>`SRC-0781` | `781` |
| <a id="src-0782"></a>`SRC-0782` | `782` |
| <a id="src-0783"></a>`SRC-0783` | `783` |
| <a id="src-0784"></a>`SRC-0784` | `784` |
| <a id="src-0785"></a>`SRC-0785` | `785` |
| <a id="src-0786"></a>`SRC-0786` | `786` |
| <a id="src-0787"></a>`SRC-0787` | `787` |
| <a id="src-0788"></a>`SRC-0788` | `788` |
| <a id="src-0789"></a>`SRC-0789` | `789` |
| <a id="src-0790"></a>`SRC-0790` | `790` |
| <a id="src-0791"></a>`SRC-0791` | `791` |
| <a id="src-0792"></a>`SRC-0792` | `792` |
| <a id="src-0794"></a>`SRC-0794` | `794` |
| <a id="src-0796"></a>`SRC-0796` | `796` |
| <a id="src-0798"></a>`SRC-0798` | `798` |
| <a id="src-0800"></a>`SRC-0800` | `800` |
| <a id="src-0802"></a>`SRC-0802` | `802` |
| <a id="src-0804"></a>`SRC-0804` | `804` |
| <a id="src-0806"></a>`SRC-0806` | `806` |
| <a id="src-0808"></a>`SRC-0808` | `808` |
| <a id="src-0810"></a>`SRC-0810` | `810` |
| <a id="src-0812"></a>`SRC-0812` | `812` |
| <a id="src-0814"></a>`SRC-0814` | `814` |
| <a id="src-0816"></a>`SRC-0816` | `816` |
| <a id="src-0818"></a>`SRC-0818` | `818` |
| <a id="src-0820"></a>`SRC-0820` | `820` |
| <a id="src-0822"></a>`SRC-0822` | `822` |
| <a id="src-0824"></a>`SRC-0824` | `824` |
| <a id="src-0826"></a>`SRC-0826` | `826` |
| <a id="src-0827"></a>`SRC-0827` | `827` |
| <a id="src-0828"></a>`SRC-0828` | `828` |
| <a id="src-0829"></a>`SRC-0829` | `829` |
| <a id="src-0830"></a>`SRC-0830` | `830` |
| <a id="src-0831"></a>`SRC-0831` | `831` |
| <a id="src-0832"></a>`SRC-0832` | `832` |
| <a id="src-0834"></a>`SRC-0834` | `834` |
| <a id="src-0836"></a>`SRC-0836` | `836` |
| <a id="src-0838"></a>`SRC-0838` | `838` |
| <a id="src-0840"></a>`SRC-0840` | `840` |
| <a id="src-0842"></a>`SRC-0842` | `842` |
| <a id="src-0844"></a>`SRC-0844` | `844` |
| <a id="src-0846"></a>`SRC-0846` | `846` |
| <a id="src-0848"></a>`SRC-0848` | `848` |
| <a id="src-0849"></a>`SRC-0849` | `849` |
| <a id="src-0850"></a>`SRC-0850` | `850` |
| <a id="src-0851"></a>`SRC-0851` | `851` |
| <a id="src-0853"></a>`SRC-0853` | `853` |
| <a id="src-0855"></a>`SRC-0855` | `855` |
| <a id="src-0857"></a>`SRC-0857` | `857` |
| <a id="src-0859"></a>`SRC-0859` | `859` |
| <a id="src-0861"></a>`SRC-0861` | `861` |
| <a id="src-0863"></a>`SRC-0863` | `863` |
| <a id="src-0865"></a>`SRC-0865` | `865` |
| <a id="src-0867"></a>`SRC-0867` | `867` |
| <a id="src-0869"></a>`SRC-0869` | `869` |
| <a id="src-0870"></a>`SRC-0870` | `870` |
| <a id="src-0871"></a>`SRC-0871` | `871` |
| <a id="src-0872"></a>`SRC-0872` | `872` |
| <a id="src-0873"></a>`SRC-0873` | `873` |
| <a id="src-0874"></a>`SRC-0874` | `874` |
| <a id="src-0875"></a>`SRC-0875` | `875` |
| <a id="src-0876"></a>`SRC-0876` | `876` |
| <a id="src-0877"></a>`SRC-0877` | `877` |
| <a id="src-0878"></a>`SRC-0878` | `878` |
| <a id="src-0879"></a>`SRC-0879` | `879` |
| <a id="src-0880"></a>`SRC-0880` | `880` |
| <a id="src-0881"></a>`SRC-0881` | `881` |
| <a id="src-0882"></a>`SRC-0882` | `882` |
| <a id="src-0883"></a>`SRC-0883` | `883` |
| <a id="src-0884"></a>`SRC-0884` | `884` |
| <a id="src-0885"></a>`SRC-0885` | `885` |
| <a id="src-0886"></a>`SRC-0886` | `886` |
| <a id="src-0887"></a>`SRC-0887` | `887` |
| <a id="src-0888"></a>`SRC-0888` | `888` |
| <a id="src-0889"></a>`SRC-0889` | `889` |
| <a id="src-0890"></a>`SRC-0890` | `890` |
| <a id="src-0891"></a>`SRC-0891` | `891` |
| <a id="src-0892"></a>`SRC-0892` | `892` |
| <a id="src-0893"></a>`SRC-0893` | `893` |
| <a id="src-0894"></a>`SRC-0894` | `894` |
| <a id="src-0895"></a>`SRC-0895` | `895` |
| <a id="src-0896"></a>`SRC-0896` | `896` |
| <a id="src-0897"></a>`SRC-0897` | `897` |
| <a id="src-0898"></a>`SRC-0898` | `898` |
| <a id="src-0899"></a>`SRC-0899` | `899` |
| <a id="src-0900"></a>`SRC-0900` | `900` |
| <a id="src-0901"></a>`SRC-0901` | `901` |
| <a id="src-0902"></a>`SRC-0902` | `902` |
| <a id="src-0903"></a>`SRC-0903` | `903` |
| <a id="src-0904"></a>`SRC-0904` | `904` |
| <a id="src-0905"></a>`SRC-0905` | `905` |
| <a id="src-0906"></a>`SRC-0906` | `906` |
| <a id="src-0907"></a>`SRC-0907` | `907` |
| <a id="src-0908"></a>`SRC-0908` | `908` |
| <a id="src-0909"></a>`SRC-0909` | `909` |
| <a id="src-0910"></a>`SRC-0910` | `910` |
| <a id="src-0911"></a>`SRC-0911` | `911` |
| <a id="src-0912"></a>`SRC-0912` | `912` |
| <a id="src-0913"></a>`SRC-0913` | `913` |
| <a id="src-0914"></a>`SRC-0914` | `914` |
| <a id="src-0915"></a>`SRC-0915` | `915` |
| <a id="src-0916"></a>`SRC-0916` | `916` |
| <a id="src-0917"></a>`SRC-0917` | `917` |
| <a id="src-0918"></a>`SRC-0918` | `918` |
| <a id="src-0919"></a>`SRC-0919` | `919` |
| <a id="src-0920"></a>`SRC-0920` | `920` |
| <a id="src-0921"></a>`SRC-0921` | `921` |
| <a id="src-0922"></a>`SRC-0922` | `922` |
| <a id="src-0923"></a>`SRC-0923` | `923` |
| <a id="src-0924"></a>`SRC-0924` | `924` |
| <a id="src-0925"></a>`SRC-0925` | `925` |
| <a id="src-0926"></a>`SRC-0926` | `926` |
| <a id="src-0927"></a>`SRC-0927` | `927` |
| <a id="src-0928"></a>`SRC-0928` | `928` |
| <a id="src-0929"></a>`SRC-0929` | `929` |
| <a id="src-0930"></a>`SRC-0930` | `930` |
| <a id="src-0931"></a>`SRC-0931` | `931` |
| <a id="src-0932"></a>`SRC-0932` | `932` |
| <a id="src-0933"></a>`SRC-0933` | `933` |
| <a id="src-0935"></a>`SRC-0935` | `935` |
| <a id="src-0937"></a>`SRC-0937` | `937` |
| <a id="src-0939"></a>`SRC-0939` | `939` |
| <a id="src-0941"></a>`SRC-0941` | `941` |
| <a id="src-0943"></a>`SRC-0943` | `943` |
| <a id="src-0945"></a>`SRC-0945` | `945` |
| <a id="src-0947"></a>`SRC-0947` | `947` |
| <a id="src-0949"></a>`SRC-0949` | `949` |
| <a id="src-0951"></a>`SRC-0951` | `951` |
| <a id="src-0953"></a>`SRC-0953` | `953` |
| <a id="src-0955"></a>`SRC-0955` | `955` |
| <a id="src-0957"></a>`SRC-0957` | `957` |
| <a id="src-0959"></a>`SRC-0959` | `959` |
| <a id="src-0961"></a>`SRC-0961` | `961` |
| <a id="src-0963"></a>`SRC-0963` | `963` |
| <a id="src-0965"></a>`SRC-0965` | `965` |
| <a id="src-0967"></a>`SRC-0967` | `967` |
| <a id="src-0969"></a>`SRC-0969` | `969` |
| <a id="src-0971"></a>`SRC-0971` | `971` |
| <a id="src-0973"></a>`SRC-0973` | `973` |
| <a id="src-0975"></a>`SRC-0975` | `975` |
| <a id="src-0977"></a>`SRC-0977` | `977` |
| <a id="src-0979"></a>`SRC-0979` | `979` |
| <a id="src-0981"></a>`SRC-0981` | `981` |
| <a id="src-0983"></a>`SRC-0983` | `983` |
| <a id="src-0985"></a>`SRC-0985` | `985` |
| <a id="src-0987"></a>`SRC-0987` | `987` |
| <a id="src-0989"></a>`SRC-0989` | `989` |
| <a id="src-0991"></a>`SRC-0991` | `991` |
| <a id="src-0993"></a>`SRC-0993` | `993` |
| <a id="src-0995"></a>`SRC-0995` | `995` |
| <a id="src-0997"></a>`SRC-0997` | `997` |
| <a id="src-0999"></a>`SRC-0999` | `999` |
| <a id="src-1001"></a>`SRC-1001` | `1001` |
| <a id="src-1003"></a>`SRC-1003` | `1003` |
| <a id="src-1005"></a>`SRC-1005` | `1005` |
| <a id="src-1007"></a>`SRC-1007` | `1007` |
| <a id="src-1009"></a>`SRC-1009` | `1009` |
| <a id="src-1011"></a>`SRC-1011` | `1011` |
| <a id="src-1013"></a>`SRC-1013` | `1013` |
| <a id="src-1015"></a>`SRC-1015` | `1015` |
| <a id="src-1016"></a>`SRC-1016` | `1016` |
| <a id="src-1017"></a>`SRC-1017` | `1017` |
| <a id="src-1018"></a>`SRC-1018` | `1018` |
| <a id="src-1019"></a>`SRC-1019` | `1019` |
| <a id="src-1021"></a>`SRC-1021` | `1021` |
| <a id="src-1023"></a>`SRC-1023` | `1023` |
| <a id="src-1025"></a>`SRC-1025` | `1025` |
| <a id="src-1027"></a>`SRC-1027` | `1027` |
| <a id="src-1029"></a>`SRC-1029` | `1029` |
| <a id="src-1031"></a>`SRC-1031` | `1031` |
| <a id="src-1033"></a>`SRC-1033` | `1033` |
| <a id="src-1035"></a>`SRC-1035` | `1035` |
| <a id="src-1037"></a>`SRC-1037` | `1037` |
| <a id="src-1039"></a>`SRC-1039` | `1039` |
| <a id="src-1041"></a>`SRC-1041` | `1041` |
| <a id="src-1043"></a>`SRC-1043` | `1043` |
| <a id="src-1045"></a>`SRC-1045` | `1045` |
| <a id="src-1047"></a>`SRC-1047` | `1047` |
| <a id="src-1049"></a>`SRC-1049` | `1049` |
| <a id="src-1051"></a>`SRC-1051` | `1051` |
| <a id="src-1053"></a>`SRC-1053` | `1053` |
| <a id="src-1055"></a>`SRC-1055` | `1055` |
| <a id="src-1057"></a>`SRC-1057` | `1057` |
| <a id="src-1059"></a>`SRC-1059` | `1059` |
| <a id="src-1061"></a>`SRC-1061` | `1061` |
| <a id="src-1063"></a>`SRC-1063` | `1063` |
| <a id="src-1065"></a>`SRC-1065` | `1065` |
| <a id="src-1067"></a>`SRC-1067` | `1067` |
| <a id="src-1069"></a>`SRC-1069` | `1069` |
| <a id="src-1071"></a>`SRC-1071` | `1071` |
| <a id="src-1072"></a>`SRC-1072` | `1072` |
| <a id="src-1073"></a>`SRC-1073` | `1073` |
| <a id="src-1074"></a>`SRC-1074` | `1074` |
| <a id="src-1075"></a>`SRC-1075` | `1075` |
| <a id="src-1076"></a>`SRC-1076` | `1076` |
| <a id="src-1078"></a>`SRC-1078` | `1078` |
| <a id="src-1080"></a>`SRC-1080` | `1080` |
| <a id="src-1082"></a>`SRC-1082` | `1082` |
| <a id="src-1084"></a>`SRC-1084` | `1084` |
| <a id="src-1085"></a>`SRC-1085` | `1085` |
| <a id="src-1086"></a>`SRC-1086` | `1086` |
| <a id="src-1087"></a>`SRC-1087` | `1087` |
| <a id="src-1088"></a>`SRC-1088` | `1088` |
| <a id="src-1089"></a>`SRC-1089` | `1089` |
| <a id="src-1090"></a>`SRC-1090` | `1090` |
| <a id="src-1091"></a>`SRC-1091` | `1091` |
| <a id="src-1092"></a>`SRC-1092` | `1092` |
| <a id="src-1093"></a>`SRC-1093` | `1093` |
| <a id="src-1094"></a>`SRC-1094` | `1094` |
| <a id="src-1095"></a>`SRC-1095` | `1095` |
| <a id="src-1096"></a>`SRC-1096` | `1096` |
| <a id="src-1097"></a>`SRC-1097` | `1097` |
| <a id="src-1098"></a>`SRC-1098` | `1098` |
| <a id="src-1099"></a>`SRC-1099` | `1099` |
| <a id="src-1100"></a>`SRC-1100` | `1100` |
| <a id="src-1101"></a>`SRC-1101` | `1101` |
| <a id="src-1102"></a>`SRC-1102` | `1102` |
| <a id="src-1103"></a>`SRC-1103` | `1103` |
| <a id="src-1104"></a>`SRC-1104` | `1104` |
| <a id="src-1105"></a>`SRC-1105` | `1105` |
| <a id="src-1106"></a>`SRC-1106` | `1106` |
| <a id="src-1108"></a>`SRC-1108` | `1108` |
| <a id="src-1110"></a>`SRC-1110` | `1110` |
| <a id="src-1112"></a>`SRC-1112` | `1112` |
| <a id="src-1114"></a>`SRC-1114` | `1114` |
| <a id="src-1116"></a>`SRC-1116` | `1116` |
