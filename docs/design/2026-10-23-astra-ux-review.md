# Solaris: interaction language and cross-surface UX review

Independent proposal by DesignAstra, 2026-10-23. R&D design specification, **not implemented functionality or an acceptance report**. The consolidated decision belongs in `DESIGN_LANGUAGE.md`; this file retains the independent argument and source evidence for cross-review.

## 1. Position: an instrument for entering and operating a world

Solaris should feel like a carefully made instrument, not a SaaS landing page, a sci-fi cockpit, or a recolored Minecraft skin. Its own UI has three jobs: **get me into the right world, tell me what is happening, let me safely change it**. The world remains the hero. A good screen makes the next action obvious without shouting. Branding lives in proportion, disciplined alignment, authored icons and language—not a gradient, a card around everything, or a glow on every control.

My independent visual recommendation is low-chroma charcoal, warm readable text, restrained amber for selection/action, squared rectangular controls with small corner treatment, no decorative blur. Primary action placement, grouping and clear text precede visual novelty. Opus owns the exact token specification and mockups; this recommendation is intentionally independent rather than copied from that draft.

### Scope and boundary

| Surface | We own | We must not restyle |
|---|---|---|
| Native launcher | Selection, install/repair, local profiles, launcher settings, diagnostic presentation | System dialogs, Microsoft/browser auth UI |
| Native client | Solaris-owned options, monitor capture/share/owner dialogs, custom overlays and addon chrome | Vanilla inventory/container slots, hotbar/hearts, crafting, chat and other vanilla screens/semantics |
| Fabric Solaris Loader | Solaris permission prompt, custom view renderer, operator terminal, Solaris overlays | Minecraft's underlying screens, OS permission picker |
| First-party plugins | Plugin-authored names, item labels, action wording, custom views where supported | Vanilla inventory-menu frame/slots when a plugin uses that transport |
| Server GUI / web | Dashboard shell, metrics, diagnostic navigation and our own web pages | Browser chrome, terminal conventions, third-party services |

Same language is **not identical pixel layouts**. Desktop uses readable logical pixels; the in-game adapter respects GUI scale and vanilla boundaries. The dashboard needs dense time-series comparison, the launcher needs a short path to Join, and consent needs space to understand scope. Sharing tokens is sensible; inventing a cross-language UI framework is not.

## 2. Evidence and consistency audit

Source roots used below:

- **C** = `/home/kaiserroman/wt/client-computer` (read-only active tree; observations are source snapshots, not proof the d90b9c2 release has every later change).
- **L** = `/home/kaiserroman/solaris-loader` (read-only).
- **P** = `/home/kaiserroman/solaris-default-plugins` (read-only).
- Runtime screenshots and their exact provenance are owned by DesignOpus and linked from the consolidated document. No screenshot is implied by a source-only finding.

| Evidence | Observable implementation / concrete failure scenario | Design decision |
|---|---|---|
| C `launcher/src/ui.rs:500-599` | Play shows profile and server management side by side; selected values save on each edited character. To join a saved server, the player must visually filter through Username, Name, Address, New and Remove. An accidental edit persists immediately. | Play is selection, not a configuration form. Keep identity compact. Edit/Add opens a small explicit Save/Cancel form. Preserve selection and draft separately. |
| C `launcher/src/ui.rs:402-405,471-475` | First run says INSTALL & PLAY and discloses approximate download size; good existing intent, but storage location and stage consequences are in other places. | Preserve explicit download consequence. First-run preparation shows version, sources, estimated transfer and location next to the action. Do not invent exact sizes before manifest resolution. |
| C `launcher/src/ui.rs:435-459` | Unknown-total work feeds a looping fraction into an ordinary progress bar; overall progress weights stages equally. During unpacking a bar can appear to move backward, and its appearance implies byte/time precision that is not present. | A labeled indeterminate stage indicator, not a percentage. Known download stage shows bytes. Overall shows named completed/current/upcoming steps, not invented total-time percent. |
| C `launcher/src/ui.rs:307-315,379-382` | User cancellation becomes `Cancelled`, is logged with ERROR and displayed as an error. A user stopping a download receives failure styling for their successful choice. | Canceled is neutral; failed is error. Keep last completed work visible. Do not promise resumability until the installer supports it. |
| C `launcher/src/ui.rs:414-418` | A button labeled Stop directly calls `child.kill()`. A player intending to return to the launcher could abruptly terminate the game. | Until graceful shutdown exists, label **Force close game** and explain unsaved-progress risk in confirmation. A future graceful action is **Quit game**, not a cosmetic rename of kill. |
| C `launcher/src/ui.rs:545-550` | Microsoft sign-in is disabled, with explanatory hover and “coming later.” | Persistent plain boundary: “This preview uses offline test profiles. Microsoft sign-in is unavailable.” No aspirational login path in mockups presented as shipped capability. |
| C `launcher/src/ui.rs:603-649`; C `src/ui/options.rs:30-96` | Render distance appears both in launcher and native options; most settings live in game. Desktop computer is under Controls, separate from the sharing consequences. | One setting meaning/storage owner; launcher labels “Applies next launch” where applicable. Put capture/voice privacy in a discoverable Privacy & sharing group, with links from Audio/Controls rather than duplicate contradictory toggles. |
| C `src/computer/native/sharing.rs:19-26` | Native consent names the requester, says pictures only/no input, and explicitly grants viewing **all my open screens**. Good separation of remote viewing from control; critical breadth. | Keep actual breadth in the button and adjacent scope text. A single-window mockup is not permission to narrow the wording while keeping broad authorization. |
| C `src/computer/native.rs:41-57` | Owner menu mixes Control, Change window, Stop sharing, per-viewer Revoke and Done in one list. `StopCapture` closes the capture, while revocation is a distinct action. | Separate source, local control, and viewer access. Use **Close capture** versus **Revoke all viewers**; do not overload Stop sharing for both. |
| C `src/computer/native.rs:91-99,130-132`; `sharing.rs:162-175` | Esc releases local control; Shift+Esc closes capture in context; F8 revokes/leaves viewing. These are different safety operations. | Persistent, contextual instruction with current binding and visible equivalent controls; never put all safety into a toast or require remembering F8. |
| C `src/ui/desktop.rs:63-88` | Dialog content and actions wrap, selected rows are kept visible, and keyboard hint exists. These are good foundations. The layout uses a generic list for permission decisions and owner commands alike. | Preserve wrapping/semantic actions. Consent is a bounded decision with identity/scope/action footer; owner management is a grouped panel with a scrollable viewer list. |
| C `src/ui/theme.rs:72-84,112-123` | Theme applies glow, rim, translucency and sheen to common controls/panels. Over a variable world, text depends on the composited backdrop. | One opaque-enough reading surface; border for boundaries, focus outline for keyboard, accent for action/selection. Do not use glow as the only state cue. Contrast measured over the final composition. |
| L `loader-fabric/.../SolarisFabricLoader.java:357-376`; L `README.md:70-79` | Permission prompt concatenates raw wire permission names into a sentence and offers generic Allow/Deny. Denying disconnects. A new player cannot connect technical scope names to effects or anticipate disconnection. | Named server identity, human capability descriptions, optional raw identifiers in Details. Buttons **Allow content & join** / **Decline & disconnect**, with accurate download consequence. No faux per-scope toggles if protocol decision is all-or-nothing. |
| L `loader-platform-common/.../LoaderViewScreen.java:108-118` | Presenting a new model clears page, tab, input text and selections. A server refresh while typing can erase a draft or reset navigation. | Revision safety remains mandatory. Retain only compatible UI-local draft/focus by stable field identity; visibly invalidate when authoritative meaning changes. Never replay an old action. This needs implementation/state tests, not just a theme. |
| L `loader-platform-common/.../LoaderViewScreen.java:138-167,203-206` | One view has both local layout pagination and model/data pagination. At small GUI size “Page 1 of 3” plus “2 / 4” describes two different dimensions. | One user-facing data-page control; local overflow scrolls. If pagination must remain, label **Results page** versus **Screen section** explicitly. |
| L `loader-platform-common/.../LoaderViewScreen.java:378-389,417-421` | Enum selection cycles by clicking a button; disabled action reasons are both tooltip and visible text (good). | Real select/list for multi-option choices; preserve visible deny reasons. A click-to-cycle control is not discoverable for 8 options and hides alternatives. |
| L `loader-fabric/.../ComputerScreen.java:30-35,53-92` | New terminal reply resets page to newest and rebuilds/focuses the input. Text wrapping uses `width / 6` character counts. Reading older output can be interrupted by a reply. | Keep reading position when scrolled back; show “New output” with an explicit jump action. Font-measured wrapping and Unicode-safe boundaries. Do not replace the terminal with decorative cards. |
| P `sources/basic-economy/src/lib.rs:278-288,340-370,469-500` | Shop item label compresses product, price, refund and owned count into one line. Primary buys, secondary refunds; transaction closes the menu and reports outcome later. | Preserve vanilla container chrome. Give the item a short name and explicit purchase/refund instructions in supported text; show authoritative transaction result, not immediate success. A richer custom shop is a later capability-backed slice, not assumed available today. |
| P `README.md:3-7,35-38`; `sources/solaris-towns/src/lib.rs:249-292` | Current towns package is command/chat-based. Online-roster and land-claims are legacy references, not deployable current GUIs; settlements moved to core. | Do not use old Lua screenshots to claim current plugin UI. Apply shared language to real authored output and future supported custom views, without inventing existing dashboards. |

The owner's report about text crammed into round inventory buttons is authoritative as a reported experience. It is not a reason to replace every text button with an icon. **Small repeated tool actions use icons; consequential choices use text; vanilla inventory remains vanilla.** The exact release screenshot locator belongs in the pixel audit, not fabricated here.

## 3. Information architecture

### Launcher

Top-level destinations: **Play**, **Settings**, **Diagnostics**. No separate dashboard, store, news feed or features grid.

Play hierarchy: current local identity → saved server list with address and real availability/compatibility if available → selected destination summary → one primary **Join [server]** action. Add server is secondary; Edit/Remove belongs to the selected server's context menu, with a visible accessible overflow label. Installation status occupies the same action region, not another competing hero.

Identity wording distinguishes **Offline test profile** from an authenticated account. Selecting a profile is one compact control; management is explicit. Empty server list says “Add the address of a server you want to join,” followed by Add server. No fake default online state. Search only appears when the list warrants it; it is not permanent decorative chrome for two entries.

Settings groups: Startup; Game defaults (with source-of-truth and “next launch” semantics); Content & storage; Accessibility; Advanced. Content manifest is advanced and explains trust implications, not adjacent to ordinary fullscreen choices. Diagnostics contains recent operations, clear errors and selectable technical details; Copy diagnostic summary states what it includes and redacts secrets, server-private identifiers as appropriate. Opening a folder is not conflated with copying its path.

### Native / Loader custom UI

Maintain a stable **Solaris** entry for owned features: Sharing, Addons, custom preferences. Do not reorganize vanilla pause/inventory pages merely to impose brand. Shared sections use identical names and roles between native and Fabric when the capability exists. Platform differences are stated (“Window sharing unavailable on this platform”), not filled with disabled aspirational widgets.

Custom options use category → labeled setting → current value and unit → one-line explanation. Important constraints remain visible, not exclusively in hover. Accessibility is directly reachable; it is not a hidden synonym for Interface. Restore defaults is scoped to the category and explains what changes.

### Plugins / server dashboard / website

Plugin custom view shell: feature title + originating plugin/server; resource summary; task-focused list or table; selected-object actions; close/back. “Owner” always names whether it means monitor owner, plugin namespace or server operator. Do not use one vague permission badge for all three.

Dashboard IA recommendation (source/pixel audit owned by Opus): Overview → Performance → Players/world → Events/diagnostics. Show target server/environment and sample freshness at every level. Overview answers “is this server healthy, what changed, where do I inspect?” rather than showing decorative metric tiles. Preserve context on drill-down: time range, instance, units and selected event. Configuration/destructive operations only where actually implemented; never add inert Restart/Save buttons to fill a mockup.

Own public web pages: Download / Compatibility / Documentation / Releases, with exact platform/version and preview limits. A download page answers requirements, authenticity, installation and rollback before branding. No invented testimonials, performance claims, live counters or decorative analytics screenshot. Website and dashboard are not the same audience or navigation.

## 4. Critical flows, including failure and cancellation

### F1 — First run / download

1. Show product version, preview boundary, offline-profile state, destination and install location. Sources are named; transfer is “estimated” until known. If resources already exist, say **Check existing files**, not Download again.
2. Primary **Install & join [server]** explicitly captures post-install intent. Secondary **Install only** is available from content settings. No network work on mere selection of a server.
3. Stage sequence: Resolve → Download → Verify → Prepare → Ready. Current stage has a sentence; completed stages do not become pseudo-progress. Download reports transferred/total bytes and recent speed; remaining time only if meaningfully estimated. Unknown-total stages show activity without fake percent.
4. **Cancel installation** changes immediately to **Canceling…**, then neutral **Installation canceled** after the worker terminates. Never auto-join after cancellation. Explain what was retained only from actual installer outcome.
5. Failure remains in the action region: specific failed operation, impact, safe recovery. E.g. “Could not download game content. Nothing new was installed.” **Retry download**, **Change location** for a real disk-location remedy, **Show details**. Retain selected server/profile. Do not recommend repair for an authentication failure.
6. Ready → Connecting → Waiting for server → Joined; process spawn is **Starting game**, not proof of joining. If installation completes while the user chose another server, honor the explicitly captured destination or ask them to choose; do not silently reinterpret the queued action.
7. Closing launcher: running game remains running (existing behavior). Active installation offers **Keep downloading** / **Cancel installation and close**; cancellation cleanup semantics must be real.

### F2 — Server select / connection

- Single click/arrow selects; Enter on selected row or Join activates. Selection never starts a connection by itself. Keyboard focus and selected server are visibly different.
- Add/Edit form: Name and Address; example IPv4/DNS/IPv6 presentation follows the parser's supported syntax, not a false promise. Inline validation on submit/blur; retain user input. **Save server** / **Cancel**. No saving invalid intermediate keystrokes.
- Row: name, address, status text, optional measured latency with units. “Unknown” is not “Offline.” If no status probe exists, omit it. Incompatible version explains required/actual versions and does not offer a knowingly impossible Join.
- Connecting is cancelable; late success after cancel must not open a session. Timeout/disconnect returns to the same selection with reason and concrete next action. **Retry connection** is a user decision, not an endless automatic loop.
- Removing a saved server deletes a bookmark, not server data. Say so. Use Undo for recoverable bookmark removal; never generic “Are you sure?” for every click.

### F3 — Settings

- Audio volume and ordinary numeric preferences preview immediately and persist consistently. Say “Saved automatically” once, not a green toast for each slider tick.
- Slider has label, visible numeric value/unit, arrow-key steps and a text/stepper equivalent where appropriate. Home/End use actual min/max. Drag release commits expensive work; keyboard change debounces expensive rebuilds without lying about applied state.
- Restart-required settings show **Applies next launch** at the row. A setting request failure restores the authoritative value with explanation. No silent optimism.
- Display-mode changes that can hide the UI use a timed **Keep changes / Revert** mechanism as an implementation requirement; not claimed implemented today. Focus stays on the safe choice; timeout reverts.
- Accessibility changes preview immediately with a visible reset path. High contrast changes semantics-preserving tokens, not only brightness. Reduced motion removes animated sweeps/glows/transitions while leaving status text intact.
- Leaving ordinary live settings does not prompt for nonexistent unsaved changes. Add/Edit forms and advanced configuration do have explicit transactional Save/Cancel.

### F4 — Monitor capture and viewing consent

Three independent permissions: (a) local OS capture/input, (b) local player's in-game control of their own source, (c) another player's viewing. Never conflate them or imply remote input is granted.

1. Owner chooses monitor → **Choose window**. Explain local capture scope. OS picker/consent remains authoritative on platforms that provide it. No fabricated thumbnails or list of windows before permission.
2. Opening → Active local capture. Persistent compact status: “Capturing [source] · Private” with **Manage**. If capture fails, show reason and **Choose another window**; do not leave a frozen image looking live.
3. Viewer requests access → waiting state names owner and scope; cancel is explicit. A denial is a result, not an application error.
4. Owner receives request with requester identity, server, the exact grant scope, and privacy warning: **Pictures only; no keyboard or mouse control**. Initial focus is Deny. Esc denies. No timer auto-accepts, no default checked “Remember,” no misleading shield implying authenticated trust in an offline profile.
5. Current native grant is **all open screens**. Button remains **Allow viewing all open screens** until authorization is actually narrowed end to end. If source/title list can be shown reliably, list it; otherwise display accurate breadth, not a fake exact list. Any narrower future per-monitor policy is a separately reviewed protocol/security slice.
6. Following approval, distinguish **Allowed to view** from **Watching now**; only show the latter when observable. Persistent indicator does not disappear with a toast. **Manage viewers** remains reachable without aiming at a tiny world object.
7. Stop/revoke visibly changes local status immediately to revoked/not sending, and remote completion is not falsely claimed before confirmation. Reconnect, changed identity or changed scope cannot inherit accidental broad approval. Review actual lifecycle contract before promising persistence.

### F5 — Monitor owner management

Panel title: **Your monitor**; context: chosen source and capture state. Three groups:

- **Source**: current window; **Change window**; unavailable-source recovery.
- **Local control**: status; **Control window** when granted by backend; visible **Release control**. Esc releases local input, never silently grants remote control.
- **Viewers**: names plus relevant identity context, allowed/active state if known, **Revoke [name]** each. **Revoke all viewers** retains local capture. **Close capture** stops the source entirely. Closing this panel only dismisses it.

Changing source must explain whether permissions are cleared or retained according to actual backend policy. Current native `sharing.rs:70-79` clears viewers when the announced capture identity changes; do not promise fresh permission for every window change without checking whether that identity changes. Recommended privacy policy is renewed approval for a changed source; implementing it is distinct from visual work.

Safety keys are displayed from bindings, not hardcoded prose that drifts. F8 and Shift+Esc remain visible contextual equivalents where supported. Focus loss releases held input. Long names wrap; Revoke buttons are not pushed outside the viewport. No remote viewer is called a controller.

### F6 — Addon activation, view action, transaction

- Permission prompt: server identity → human-readable capability effects → missing bundle download consequence → **Allow content & join** / **Decline & disconnect**. Raw names/hashes remain expandable details. Unknown permissions are not euphemized as “enhancements.”
- Custom view action: select object → read cost/consequence → activate → **Pending** → authoritative applied/refused result. Maintain selected object while possible; stale revision explicitly asks for refresh/reselection, never reapplies an old action to a new target.
- Refresh preserves safe UI-local reading position/drafts, not authority. Changed field meaning clears or marks the draft with explanation. Focus does not jump to the first destructive action on refresh.
- For the current inventory shop, communicate **Primary click: buy [count] for [price]** and **Secondary click: refund [count] for [amount]**, subject to supported label/lore transport. On commit report actual result; on refusal explain changed inventory or terms. Do not overlay custom rounded buttons on vanilla slots.
- Operator terminal: clear target, input history and readable output; while reading older messages, new output increments an indicator instead of resetting position. No need for a “chat card” around every line.

### F7 — Error language across every surface

Format: **What happened → what it means → what you can do**. Visible concise message, optional selectable/redacted technical details. One primary remedy; secondary navigation. Errors remain until resolved/dismissed; success toasts may expire. Offline, canceled, denied, unavailable and failed are distinct states.

Examples are proposed copy, not observed runtime messages:

| Situation | Proposed copy / actions |
|---|---|
| Content network failure | “Download stopped. Verified files are still available.” Only if retention is confirmed. Retry download · Details |
| Disk full | “Not enough space to prepare game files.” Required/free only if measured. Choose location · Details |
| Server mismatch | “This server requires a different game version.” Actual versions when known. Back to servers |
| Capture revoked by OS | “Window capture ended. Other players are no longer receiving new pictures.” Choose window · Dismiss; stale remote frame clearing must be verified |
| Permission denied | “Viewing was not allowed.” Back; no red fatal-error panel |
| Stale addon action | “This view changed before the action could be applied.” Refresh view; never “Success” on enqueue |
| Dashboard stream stale | “Updates disconnected. Last sample at [time].” Retain old graph with a stale marker, not a line dropping to zero |

## 5. Component interaction contract

Every component has default, hover, keyboard focus, pressed, disabled, busy, error and selected states where meaningful; not all states need different surfaces. Hover must not move neighboring layout. Focus is a high-contrast outline independent of selection, visible on dark/light/world backgrounds.

| Component | Required behavior |
|---|---|
| Text button | Verb + object for meaningful action; one primary per decision region; destructive separated spatially. Disabled reason visible/focus-readable. Busy preserves width and prevents duplicate submission. |
| Icon button | Only familiar repeated tools (close, back, copy, volume, settings); authored icon, tooltip on hover **and focus**, accessible name, visible focus, sufficiently sized hit area. Grant, delete, pay, join and stop capture use text. Do not squeeze words into icon-sized circles. |
| List / server row | Full selection row is a single target; nested overflow has its own non-overlapping target; stable selection while data refreshes; keyboard navigation scrolls into view; status not color-only. |
| Dialog | Title, focused task, visible close/cancel, bounded scroll body, reachable footer. Trap focus; inert background; return focus to invoker. Safe initial focus for grants/destruction. Esc never approves. |
| Toast / notice | Short confirmation for reversible non-critical events; no permission decisions, crash root cause or sole safety control in a transient toast. Can be paused/read; announce once. |
| Slider | Label + value + unit; pointer, keyboard, stepper/text alternative; focus and active thumb; numeric bounds stated when relevant. Never “quality 73” without meaning. |
| Progress | Distinguish known-total fraction, unknown-total activity and completed steps. Cancellation has its own intermediate/final state. No decorative percentages. |
| Table | Real column headers, aligned numerics, explicit units/sort state, empty/error/loading distinct. Windowed rows retain accessible total/position. Data pagination separate from viewport scrolling. |
| Status strip / HUD | Compact, fixed safe region, readable backing; state icon plus text; ownership/scope shown when privacy relevant; reserve space away from vanilla hotbar/chat/subtitles. Non-modal HUD never steals game focus. |
| Settings toggle | Immediate boolean state, not a navigation button; label describes the enabled state; explain permission/device failure alongside it. |

## 6. Accessibility and localization acceptance targets

These are proposed release gates, not claims that current UI passes WCAG or OS accessibility.

- **Semantics:** expose name, role, state/value, description, bounds, focus and disabled reason through the real native/Fabric accessibility path. Existing MCP semantic trees do not automatically prove screen-reader support. Browser dashboard uses semantic HTML before ARIA; native adapters need platform accessibility/narration verification.
- **Keyboard:** all custom actions reachable without pointer; predictable Tab/Shift+Tab, arrows inside lists/tabs, Enter/Space activation, Esc cancel/close; no keyboard trap except bounded modal focus with exit. No background movement after dialog consumes input. Test held-key release on focus loss and after monitor control.
- **Focus:** deterministic entry and restoration; focus does not vanish after list deletion or asynchronous refresh. Consent starts on Deny or readable title, not Allow. Pending content never steals focus.
- **Targets:** default desktop controls 36–40 logical px high; dense tools at least 32×32 with separation; important consent/safety controls 40–44. WCAG 2.2 AA's web minimum is 24×24 CSS px with defined exceptions, not a mandate that every Minecraft GUI-pixel control be 24. Measure **effective rendered size** under supported game/OS scales. Do not make low-resolution menus unusable by blindly copying desktop dimensions.
- **Contrast:** aim at least 4.5:1 normal text, 3:1 large text and meaningful UI boundaries/focus against adjacent colors; measure every state and final composited background. Disabled controls remain understandable even where standards exempt them. Error, warning, selection, success have text/icon/shape alternatives.
- **Scale/reflow:** launcher at current minimum 820×540 and common 980×620, plus 1280×720/1920×1080 and 150/200% OS scaling; in-game at 320×240 logical and 427×240 logical plus larger sizes. At high zoom, collapse sidebar to a labeled nav choice; keep action footer reachable; scroll body instead of clipping or shrinking body type. Dashboard reflows narrow screens without discarding units/context.
- **Text:** use real Russian/Cyrillic, long German-like labels, CJK, mixed-direction server/user text, combining marks and IME composition. Scope and destructive labels wrap; never truncate the only warning. Truncate middle of technical paths/addresses only when full value can be inspected/copied. Plain body text remains selectable where useful.
- **Motion:** no idle shimmer, breathing glow, parallax or looping decoration. Brief 80–160 ms state transition only where it clarifies change; reduced motion replaces animation with a stable state. Busy text remains. Never delay permission revocation for an exit animation.
- **Sound:** quiet optional click on accepted action, not hover; no toast chime spam; error/success always visible too. Privacy and warning sounds cannot be the only cue. UI volume/mute respect the existing audio categories; do not bypass mute for branding.
- **Narration:** title/context once on modal entry, changed value on deliberate adjustment, progress milestones rather than every byte/sample. No repeated server refresh announcements over the player's reading.

Useful primary references: [W3C target minimum](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html), [W3C modal-dialog focus pattern](https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/). Their browser mechanics inform the interaction contract; native implementation still requires its own testing.

## 7. Research: what “AI slop / vibe-coded UI” means, and what never ships

Searches performed: `"vibe coded UI" "design"`, `"AI slop UI" OR "generic AI design" OR "gui slop"`, `"gui slop"`, and `"generic AI design" UI purple gradients cards`. The exact phrase “gui slop” produced weak/no relevant primary UI analysis; it is not a technical category. Do not dress that search up as scientific evidence.

Read sources, with their limits:

1. [Kate Moran, NN/g — GenUI vs. Vibe Coding: Who’s Designing? (2026-03-27)](https://www.nngroup.com/articles/genui-vs-vibe/). Distinguishes who makes design decisions; evaluates usefulness rather than screenshot fluency. Its real generated trip-planner examples are a useful comparison of unspecified versus specified structure, **not a controlled usability study**.
2. [InterfaceKit — What is AI slop in UI design?](https://blog.interfacekit.io/what-is-ai-slop-ui-design), updated 2026-06-27. Names weak product fit, unverified material, unresolved behavior and missing review. Commercial design-kit publisher; useful critique, not independent prevalence data.
3. [InterfaceKit — What makes a website look AI-generated?](https://blog.interfacekit.io/what-makes-a-website-look-ai-generated), updated 2026-09-07. Substitution, decoration-off, second-screen, uncomfortable-state and product-language tests. Explicitly says a palette/font cannot prove AI authorship.
4. [Anant Jain, Designpixil — AI Slop: Why Everything Designed With AI Looks the Same](https://designpixil.com/blog/ai-slop-design), updated 2026-09-28. Concrete recurring visual motifs: gradient hero, three equal feature cards, badge pills, generic centered copy, repeated radius/shadow, default typography. Commercial design consultancy; its causal/model and prevalence claims are not adopted as established facts here.

### Published screenshot comparisons (inspected, linked rather than redistributed)

- [NN/g vague-prompt trip planner screenshot](https://media.nngroup.com/media/editor/2026/03/18/vague-vibe-coding-trip-planner.jpg): three equal destination cards with pictograms, large whitespace, repeated activity rectangles, muted category pills. This visibly illustrates generic repeated card structure; the image alone cannot prove broken controls or poor accessibility.
- [NN/g detailed-prompt trip planner screenshot](https://media.nngroup.com/media/editor/2026/03/18/specific-vibe-coding-trip-planner.jpg): spatial map, timeline and selected-place detail have distinct jobs. It still visibly contains placeholder photo panels/map-like artwork. More tailored composition is **not** evidence that the data/backend or usability is complete.

These third-party images remain linked at their publisher. They are references, not Solaris assets, and no permission to redistribute is implied. Solaris audit screenshots are separately provenance-labeled by Opus.

### Explicit rejection rules

| Never ship | Why it fails for Solaris | What replaces it |
|---|---|---|
| Text crammed into small round tool buttons | Neither readable text nor recognizable icon; localization breaks immediately | Authored icon + accessible name/tooltip for a repeated tool; full text button for a decision |
| Every noun in a pill, every line in a card | Equal emphasis destroys hierarchy and spends space on framing | Plain grouped rows, rules and spacing; a card only for a selectable entity/context |
| Purple/amber glow, glass and gradient as the whole identity | Color swap leaves generic structure; blur weakens world-background legibility | Solid reading surfaces, measured type, clear role contrast, specific content |
| Three feature cards / “Fast. Simple. Powerful.” hero in launcher | Marketing consumes the path to joining a server | Current destination, readiness, one Join action |
| Fake dashboard numbers or decorative sparkline | Operator may act on an invented state | Real source, unit, time window, last sample, unknown/stale state |
| Disabled coming-soon controls as the main workflow | Promises what cannot be done and makes dead ends look like choice | Visible capability boundary and a usable supported path |
| Happy-path-only mockup accepted as design complete | Cancel, deny, stale, overflow and failure are normal use | State matrix and named recovery per critical flow |
| Icon-only consent, revoke, delete or dangerous stop | Ambiguous consequence; memory/tooltip dependency | Verb + object + scope with safe focus |
| Status only in color, sound or hover | Excludes users and fails in muted/low-contrast conditions | Text + shape/icon, focus-readable descriptions |
| Same card layout for dashboard, consent and server list | Superficial consistency ignores different decisions | Shared tokens/roles, task-appropriate structure |
| Toy command copy (“Oops!”, “Something went wrong”) | Hides cause and remedy; trivializes privacy/data risk | Concrete event, impact, next action, optional technical details |
| Invented trust symbols, testimonials, compatibility or speed claims | Visual certainty exceeds verified product state | Honest preview boundary and actual evidence |
| Novel asymmetric layout solely to look “not AI” | Taste theater can harm predictability as much as a template | Asymmetry only when information hierarchy earns it |
| Recoloring vanilla screens wholesale | Violates scope and learned Minecraft interaction | Solaris-owned chrome only; vanilla stays vanilla |

A screenshot is evidence of appearance, not behavior. A gradient, Inter, a round corner or a component library is not proof of authorship. “Slop” here means shipping unexamined decisions, not using a particular tool.

## 8. Implementation slices and acceptance scenarios

All code is future work; this deliverable creates new design files only. Do not block the ongoing release on this R&D plan.

| Slice | Owner boundary / changes | Observable acceptance |
|---|---|---|
| A — Baseline and tokens | Capture frozen launcher/native/Loader/dashboard states; adopt shared role/token names and icon source assets; explicit vanilla exclusions | Captures have build/platform/viewport provenance; same role can be mapped across egui, native renderer, Java and web without replacing domain logic |
| B — Core controls and accessibility | Buttons/icon buttons/focus/fields/lists/dialog/scroll containers in each owning renderer | Keyboard-only operate representative form/dialog; actual screen reader/narrator checks; long label/200% scaling/contrast/reduced-motion cases |
| C — Launcher selection and install | Separate browse/edit state, honest installation/cancel/progress, diagnostics and force-close labeling | Fresh install, cached install, cancel, download failure, invalid address, server removal and child exit all have correct reachable recovery; no phantom success |
| D — Monitor privacy surfaces | Capture picker, exact-scope consent, owner panel, persistent state, revoke vs close | Grant/deny/Esc/request expiry/source change/focus loss/revoke-all/disconnect; no picture/input disclosure beyond actual permission; safety paths remain reachable at small viewport |
| E — Custom options and HUD | Unified setting semantics, direct Accessibility/Privacy entry, custom overlay safe zones | Values apply/persist/fail honestly; scale/reduced-motion/narration; vanilla UI pixels and inputs unchanged |
| F — Loader and plugin presentation | Shared capability language, custom view layout/draft policy, terminal scrolling, current shop copy | Long permission list, deny-disconnect, stale revision, server refresh while editing, multiple result pages, refused transaction; no unauthorized action replay |
| G — Dashboard and web | Token/typography adaptation, task-based IA, honest charts/status/empty states, download/compatibility page content | Keyboard navigation, source/time-unit fidelity, stream disconnect, empty/stale/error, narrow viewport; no control exists without an implemented action |

Each slice needs ordinary semantic-path tests plus real screenshots/interaction evidence on its affected adapter. Privacy and transaction slices additionally need authoritative state evidence. No cargo/Gradle/client run was performed by this UX author; source reads and cited-image inspection are research, not runtime validation.

## 9. Cross-review questions / initial disagreements to resolve

1. **Icons versus labels:** reject an “icons everywhere” interpretation. Recommend icon-only for known repeat tools; text for scope/consequence. Vanilla inventory remains unchanged.
2. **Persistent Apply footer versus immediate settings:** recommend immediate ordinary preferences with clear status; Save/Cancel only for transactional forms/advanced edits. Do not add an Apply button that does nothing.
3. **Single-window consent versus current broad grant:** recommend narrow per-source grants as a future security change, but current-language design must accurately disclose all-open-screens scope. A mockup cannot silently change authorization.
4. **Warm identity versus all-amber:** keep a warm accent, not amber borders and glowing controls everywhere. Focus/selection/error require independent meaning.
5. **Privacy indicator versus hidden HUD:** recommend safety status remains visible while sharing even when ordinary custom HUD is hidden; whether Hide HUD explicitly hides it must be an intentional documented choice, never accidental.
6. **Aspirational mockups versus release truth:** every fabricated example value labeled illustrative. Architecture source and release screenshot timestamps are separate. Do not imply unobserved Fabric/dashboard runtime coverage.

DesignOpus accepted all six recommendations above in the cross-review exchange. The consolidated document is the single recommendation; this independent draft remains its evidence appendix.

### Cross-review exchange: agreed visual amendments

DesignAstra reviewed Opus's independent token/interaction proposal; DesignOpus explicitly accepted these amendments via peer message:

- Desktop icon targets 32 px, ordinary controls 36 px, primary/safety controls 40–44 px. Compact 24 px only in dense tables with at least 8 px separation. In-game effective hit sizing is reviewed separately from raw GUI pixels.
- IBM Plex Sans Semibold for headings. The authored pixel face belongs only to the short wordmark/splash, never consent/dialog copy.
- Keyboard focus uses a 2 px amber outer ring with 2 px offset. Selected rows use a 3 px left marker plus surface fill. They are distinct even when both use amber.
- Sharing uses signal blue, broadcast icon and explicit “Sharing · N allowed” wording. Red is reserved for danger/errors; allowed viewers are not falsely reported as currently watching.
- The agreed surface palette is warm graphite (`#0F0D0B`, `#181512`, `#221E1A`, hover `#2D2823`), with restrained amber `#FFA32E`, no amber container borders, glow or decorative gradients.

[Monitor state-contract sheet](ux-state-contract.svg) is an original interaction specimen, not a runtime screenshot or an additional implementation claim. Its desktop type illustrates the state contract; in-game adaptation retains the approved game-font/scale policy.

### Additional pixel review

DesignAstra inspected Opus's actual [first-run launcher capture](audit/launcher-play-first-run.png), [manifest error capture](audit/launcher-error-manifest.png), and [install-progress capture](audit/launcher-installing.png). The first visibly devotes two large bordered regions to inline profile/server editors with large empty lower areas. The error capture visibly wraps a raw network/URL chain into the bottom action region, competing with Install & Play. The install capture shows two progress bars but no textual explanation of how the narrow lower bar is derived. These pixel observations support separating selection from editing, short actionable error summaries with Details, and honest named-stage progress. A still screenshot alone does not prove a stall duration, actual retry behavior, or keyboard accessibility.
