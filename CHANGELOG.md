# Changelog

## 0.16.0

- Add `muse-acp logout`, which runs `muse logout` in the terminal and
  reports its exit status, for the same OS user the adapter runs as, so
  clearing the stored Muse login no longer needs an ACP client or knowledge
  of Muse's CLI. Muse owns the credential, and the adapter only hands over
  the terminal.

- Run commands in ACP v1 editor terminals (#311). When the client
  advertises `terminal`, the adapter's `editor` MCP server adds
  `run_in_terminal`, which runs the command line in an editor terminal
  (`terminal/create`, `wait_for_exit`, `output`, `release`), shows that
  terminal live on the tool card, and returns the output and exit status to
  the model. Read-only and Plan sessions refuse it, `cwd` stays inside the
  session's roots, and `session/cancel` or the call's timeout kills the
  command. Editor-tool approval prompts now name the file or the full command
  from the approval's `rawArgs`, retitling an open prompt when Muse 1.4.8
  sends them late. MCP tool cards take their kind from the tool's own name,
  so `mcp__editor__run_in_terminal` is `execute`, not `edit`.

- Read and write files through ACP v1 editors (#310). When the client
  advertises `fs.readTextFile` or `fs.writeTextFile`, each Muse session gets
  an adapter-owned MCP server, `editor`, with `read_text_file`,
  `write_text_file`, and `edit_text_file` tools that go through
  `fs/read_text_file` and `fs/write_text_file`. The model sees unsaved
  buffers, and the editor applies and tracks the writes. Paths stay inside the
  session's roots, Read-only and Plan sessions refuse writes, completed writes
  show a `diff`, and `session/cancel` withdraws a waiting request. The server
  listens on 127.0.0.1 behind a per-session bearer token and refuses browser
  requests. Needs Muse 1.3.0+; `MUSE_EDITOR_TOOLS=0` turns it off.

- Correct stale vendored-surface labels after the 1.4.11 re-pin. The
  `--selftest` and handshake detail for the Muse 1.4.8 fingerprint no longer
  calls it "the vendored SDK manifest surface" beside the 1.4.11 row that
  now is; it names the previously vendored `838d36a` surface. ROADMAP's 1.4.8
  and 1.4.10 notes now compare against the then-vendored bundles.

- Pin Muse 1.4.11-R6667.1 in the nightly live-host matrix and classify the
  vendored 1.4.11 surface as tested (#337): the full suite on Linux x86-64,
  the newest-pin leg on macOS arm64 (taking over from 1.4.8), and the
  workspace-root flows on Windows x64. Two host-side gaps are tolerated with
  evidence: 1.4.11's Linux build refuses `session/fork` and
  `session/sideChat` because it cannot read a file's creation time (on
  GitHub's runners too), so those two live tests skip on exactly that
  refusal; and it refuses a workflow child agent call under
  `reasoning_effort: none`, so the workflow-child live test names a
  supported effort.

- Keep MSP command ids random and unique on Windows. Without
  `/dev/urandom` the ids carried almost no randomness, and only four bits of
  the per-host sequence reached `rand_a`, so ids minted 16 apart in one
  millisecond collided; Muse treats command ids as idempotency keys. Ids now
  draw on std's OS-seeded `RandomState` when `/dev/urandom` is missing, and
  the sequence's low 12 bits keep ids distinct within a millisecond even with
  no entropy. Found by the new client crate's tests on the Windows nightly
  legs.

- Log in through the browser with ACP URL elicitation (#315). Clients that
  advertise `elicitation.url` get an agent-handled auth method,
  `muse-device-login`. ACP v2 `auth/login` (or v1 `authenticate`) with it
  starts Muse's device-code login (experimental `account/loginStart`), sends
  the verification page as a request-scoped URL elicitation with the code to
  confirm, and answers once Muse's `account/loginCompleted` arrives, after
  `elicitation/complete`: `{}` when the login is granted, `-32000` otherwise.
  Declining the page cancels the flow through `account/loginCancel`. No
  credential passes through ACP.

- Relay Muse 1.4.11 recovery observations on host errors (#334). A refused
  resume, load, fork, or prompt whose host error carries a complete
  `recoveryObservation` now keeps it in the ACP error's
  `data.recoveryObservation` and logs one diagnostic line; an approval
  decision failure logs it as well. Incomplete observations are dropped,
  and the qualification is never read as acceptance. The recovery choice
  (`recoveryMode`) stays unsent: ACP has no recovery concept to expose, and
  omission keeps Muse's retained or legacy behavior.

- Count Muse 1.4.11 monitor inbox events on the monitor card (#330).
  `monitor/inboxItemAdmitted`, `...Drained`, and `...Voided` used to take the
  unhandled-notification path; they now keep per-monitor counts in the
  monitor card's `_meta.muse.monitorInbox {pending, drained, voided}`,
  settling a drain and a void of one item latest-wins on their recorded
  stamps, as Muse's own fold does. Voids stay count-only, with no transcript
  row.

- Surface the Muse 1.4.11 subagent `drained` join fact (#336). When the host
  reports that the parent consumed a subagent's result, the subagent card
  says "Delivered to the parent agent." and carries
  `_meta.muse.subagent.drained: true`. Card completion still follows the
  item's own status; the adapter never derives the join itself.

- Release closed sessions in Muse 1.4.11 (#329). After ACP `session/close`
  stops a session's work and drops its local state, the adapter sends
  `session/leave` with an explicit `stop: true`, so Muse unloads the session
  instead of holding it until it idle-unloads. The `session/leaveCompleted`
  terminal is correlated by `commandId` and logged with its outcome and
  reason. The session stays in `session/list` as not loaded and a resume
  reloads it. Older hosts are unchanged.

- Document and pin Muse 1.4.11 goal timestamps (#333). Goal metadata
  carries `createdAtMs` and `updatedAtMs` verbatim when Muse records them and
  omits them otherwise, never reporting a fabricated zero; a fake-host test
  pins both cases. The adapter keeps the verbatim epochs rather than
  rendering an age.

- Fix two nightly failures introduced this cycle. A `src/diff.rs` test helper
  used only by Unix tests was dead code on Windows, which failed clippy on
  both Windows legs. The `muse-msp-client` live test waited only for the
  `approval/request` server request, which Muse 1.3.0 never sends (it
  announces approvals with the `approval/requested` notification alone), so
  the 1.3.0 leg timed out; it now decides each approval once from whichever
  arrives.

- Render Muse 1.4.11 `scheduledFire` items (#332). A scheduled cron fire
  becomes a "Scheduled job <jobId> fired" card with its occurrence number,
  its scheduled and next fire times in UTC (or "Final delivery" when the
  occurrence facts carry no next fire), and the occurrence facts verbatim in
  `_meta.muse.scheduledFire`. It used to fall back to a generic card.

- Render Muse 1.4.11 `damagedRange` items (#331). A damaged session-log
  range becomes a "Damaged session log range" card stating the records
  proven missing, the unreadable lines, the readable window around the
  range, and the safe fork cut (`lastGoodTurnId`), with the range object in
  `_meta.muse.damagedRange`. It used to fall back to a generic card.

- Name the picked skill copy on Muse 1.4.11+ (#335). A native skill part now
  carries the `skill/list` row's `source` (`bundled`, `user`, `project`, or
  `plugin`, checked against the closed vocabulary) when the catalog holds
  exactly one copy of that selector, so the host resolves that exact copy.
  Duplicate selectors, older hosts, and rows without a valid source keep the
  bare-selector resolution.

- Show shell output through ACP v2 agent-owned terminals (#306). A `bash`
  tool call or user shell that runs gets a terminal: the card's content
  references it, `terminal_update` carries the command (and an explicit
  `workdir`), output streams as base64 `terminal_output_chunk` within the
  display budget, and the final update replaces the output with the bounded
  final text and sets `exitStatus` (a user shell's exit code, with its signal
  number in `_meta`; a `bash` call records no exit code, so its terminal is
  only marked exited). A call refused or cancelled before it ran keeps its
  text card, and ACP v1 is unchanged.

- Append streamed tool output with ACP v2 `tool_call_content_chunk` (#307).
  Muse streams a tool or shell card's `visibleOutput` as `item/delta` on the
  `output` field, which the adapter used to drop, so running cards stayed
  empty until completion. ACP v2 now appends each delta to the card, up to
  the display budget; the completion still replaces the whole content with
  the bounded final output, so clients end with what a replacement alone
  would show. ACP v1 is unchanged.

- Extract the adapter's Muse Session Protocol client into the workspace-local
  `muse-msp-client` crate (#328), so tools can drive `muse serve` without any
  ACP code. It launches a host from an explicit configuration (executable,
  arguments, host flags, environment, working directory, capabilities),
  completes the handshake with typed grant, fingerprint, and durability
  facts, correlates requests with per-method deadlines, cancels a pending
  request through a cloneable handle, routes server requests, classifies host
  exits, and shuts down within a bounded time. It has no dependencies, so the
  `muse-acp` binary still ships no third-party code; the JSON parser moved
  into it. `muse-acp` now drives its hosts through the crate with unchanged
  frames, logs, timeouts, Muse discovery, and settings handling. Hermetic
  tests use an in-memory transport and a self-executing fake host; an
  env-gated test runs the whole flow against a real `muse serve`.

- Send ACP diff content for Muse file edits (#301). On ACP v2, a completed
  tool call with a stored patch (`patchRef`, `kind: "tool_patch"`) gains a
  `diff` content item after its text output: `changes` names each file as
  `modify` (or `add` for a whole-file write that replaced no prior lines),
  and `patch` is `git_patch` text with absolute paths, built from the patch
  document read through `item/readOutput` (whose layout was established
  against a live Muse 1.4.11). Calls without a stored patch, unrecognized or
  oversized patches, history replays, and ACP v1 keep the text card; no diff
  is ever synthesized.

- Send ACP tool-call `locations` for follow-along (#302). Cards for Muse's
  file tools (`read_file`, `write_file`, `edit_file`, and the older
  single-path and move tools) carry their explicit path arguments as
  absolute paths, with relative paths resolved against the session's
  workspace root, on ACP v1 and v2. Shell, search, MCP, and unknown tools
  carry no location: a path is never inferred from a command line or tool
  output.

- Send one-shot user messages as ACP `notice` updates (#308). The note that
  Muse declined to retract a submitted message is now an `info` notice on
  ACP v2 and for ACP v1 clients that advertise
  `clientCapabilities.session.notices`; other v1 clients still get it as
  agent text. Persistent hints stay clearable `_meta.muse` rows, and
  Auto-review denials, the deny-safe permission fallback, and rejected
  approval decisions stay in the transcript, since a notice may be ignored.

- Give question form fields their header as `title`, and keep option
  descriptions and previews (#324). The form message is now the question
  text alone instead of `header: question`, so clients no longer need to
  split it to label the field. Each option's Muse `description` becomes the
  titled choice's `description`, and its `preview` (`{content, format}`)
  rides the choice's `_meta.muse.preview`. Deduplicated labels still answer
  with the host's original label.

- Offer Auto-review as an ACP boolean config option (#309). ACP v2 clients,
  and ACP v1 clients that advertise
  `clientCapabilities.session.configOptions.boolean`, get a
  `type: "boolean"` toggle; other v1 clients keep the `off`/`on` selector.
  `session/set_config_option` accepts `type: "boolean"` values for it (and
  still the `off`/`on` ids), and refuses a boolean for any other option with
  `-32602`.

- Document `session/set_model` as a non-standard compatibility method (#305).
  It is the legacy unstable ACP v0.10 `SetSessionModelRequest`, absent from
  every current schema, and is kept for older clients and scripts; the
  adapter never returns the legacy `models` session state that makes
  compliant clients call it. A test now pins that the standard `model`
  selector follows a change made through it.

- Implement ACP v2 `auth/logout` and the optional ACP v1 `logout` (#303),
  now advertised as `agentCapabilities.auth.logout`. Logout calls Muse's
  experimental `account/logout` and succeeds only once `account/read`
  reports no credential, so new sessions fail with `-32000` until the user
  logs in again. A `META_API_KEY` in the agent's environment, a host without
  `account/logout`, or a credential that remains fails the logout with an
  explanation. `auth/login` answers `-32602` instead of `-32601`: the only
  advertised method is terminal auth, which clients must not pass to it.

- Advertise the typed terminal auth method only to clients that support
  terminal auth (#304). ACP v1 clients must send
  `clientCapabilities.auth.terminal: true`; without it they still receive
  `muse-login` with the legacy `_meta["terminal-auth"]` command (which Zed
  and the ACP Registry check read) but no `type: "terminal"`. ACP v2 clients
  must send `capabilities.auth.terminal: {}`; without it `authMethods` is
  empty, and auth-required errors ask the user to run `muse login` instead of
  naming a login button.

- Enforce ACP v2 `session/resume` replay cursors and state snapshots (#322).
  `replayFrom` must be omitted, `null`, or `{"type":"start"}`: an unsupported
  cursor type (such as a custom `_`-prefixed one) or a malformed value (a bare
  string, `{}`, a non-string `type`) is refused with `-32602` before any host
  call, instead of resuming with replay anyway. An explicit `null` no longer
  replays history. Every successful v2 resume now sends a current-state
  `state_update` after any replay and before the response, derived from open
  requests and tracked turns when Muse omits the session status.

- Return the inserted user message's `messageId` from every successful ACP v2
  `session/prompt` (#321), including local commands such as `/goal`, `/plan`,
  `/compact`, `/recap`, `/feedback`, `/cua`, and `/skill-import`. The
  `user_message` update now carries the same id instead of a separately
  minted one, and repeated prompts receive distinct ids. A local command
  handled from idle reports `running` before its `idle(end_turn)`, and a
  `/feedback` form stopped or closed before its prompt was inserted answers
  `-32800` instead of an empty success. `muse-acp-client` exposes the receipt
  as `PromptResponse::message_id`.

- Handle ACP v2 JSON-RPC batch arrays (#319). An empty batch receives one
  `-32600` object with `id: null`; each valid entry is dispatched as if it
  arrived alone; invalid entries receive their own `-32600` without failing
  their siblings; responses ready when the batch has been dispatched leave as
  one array; and notification-only batches receive no reply. Previously a
  batch was silently dropped. ACP v1 batches are refused with one `-32600`
  instead of being dropped.

- Re-pin the vendored Muse SDK conformance corpus to `22a8a56` (2026-10-19,
  "remirror: 1.4.11"), which publishes the Muse 1.4.11 stable surface. The
  additions over 1.4.9/1.4.10 are additive: the `session/leave` command with
  its `session/leaveCompleted` terminal and the `leave` reason on
  `session/closed`, the monitor-inbox trio (`monitor/inboxItemAdmitted`,
  `monitor/inboxItemDrained`, `monitor/inboxItemVoided`), the `damagedRange`
  and `scheduledFire` item kinds with their item fields, goal creation and
  mutation timestamps, recovery observations and modes, the skill-invocation
  source, and the workflow-child `drained` join fact. The vendored bundle is
  byte-identical to what an installed Muse 1.4.11 exports with
  `muse schema generate-json-schema`. The adapter tolerates the additions
  unchanged (inbound copies of the new notifications take the
  unhandled-notification diagnostic path and never settle a prompt; the new
  kinds flow through the unknown-kind card path; the goal timestamps ride
  the raw goal metadata), so this re-pin is documentation-only: the schema
  corpus, the compatibility table (the new fingerprint classifies as
  degraded until a pinned 1.4.11 build joins the live-host matrix, tracked
  in #337), `PROVENANCE.md`, the ROADMAP reference snapshot, sections 1, 2,
  and 9, the event matrix, and the field matrix. Each new surface the
  adapter does not drive has a tracking issue, #329 through #336.

- Validate ACP initialization before launching Muse (#320). Missing,
  non-integer, negative, or out-of-range protocol versions receive `-32602`;
  unsupported versions receive `-32022` with `requested` and `supported`
  data; requests above the supported range select the highest version at or
  below the request; and ACP v2 requires a well-formed client `info` object.

- Put the **Explain instead** choice and clarification text directly inside
  option-bearing Muse question forms. Answering a normal question now takes
  one form submission instead of a separate workflow-choice form first;
  choosing the inline explanation still sends `userInput/clarify`.

- Add the workspace-local `muse-acp-client` crate, an independent synchronous
  Rust SDK for driving ACP v1 or v2 agents over stdio. It provides process and
  custom transport ownership, initialization, session and prompt helpers,
  request correlation and timeouts, streaming events, agent-cancelled
  requests, stderr capture, and a configurable reverse-request handler. The
  default handler fails closed for permissions, elicitation, filesystem, and
  terminal access. Black-box tests drive the existing `muse-acp` binary over
  both protocol versions.

## 0.15.0

- Build release binaries with Rust 1.99.1 (previously 1.98.1). The minimum
  Rust version for source builds stays 1.88.

- Fix post-0.14.0 review findings. An auto-review verdict is discarded when
  its approval reached `approval/resolved` while the reviewer turn ran, so a
  terminal approval is never decided again or failed closed against live
  work. A resolved-approval history read that returns no page keeps the walk
  owed for the next approval instead of disabling the #238 check for the
  attachment. `session_info_update` omits an unchanged `title` on
  generated-name-only and origin-only changes, since ACP reads an explicit
  `title: null` as a clear.

- Refresh the README adapter comparison: add Muse Spark Code and
  1broseidon/muse-acp, and update npm adoption to the 30 days ending
  2026-10-18.

- Publish a complete ACP extension contract and adopt standard protocol
  surfaces (#294). `docs/acp-extensions.md` now centralizes negotiated
  methods, `_meta` shapes, capabilities, fallbacks, and standard mappings.
  ACP v2 and negotiated ACP v1 clients receive standard `compaction_update`
  instead of a synthetic tool card, with v1 fallback retained. Actual tool
  calls carry Muse's programmatic tool name as ACP `ToolCall.name`, and
  `session_info_update` publishes a complete Muse metadata snapshot so ACP v2
  top-level patches cannot erase unrelated session facts. Usage metadata now
  has canonical nested `muse.cumulative`, `muse.pressure`, and
  `muse.subscriptionUsage` members while retaining the early flat keys.

- Adopt the Muse 1.4.8 resume permission-profile pick (#257). ACP
  `session/load` and `session/resume` accept a non-empty
  `_meta.muse.permissionProfile`; the adapter forwards it only to MSP
  `session/resume`, including the MCP-configuration conflict retry. Omitting
  it preserves retained-authority behavior, malformed values fail locally,
  and unknown or unavailable ids keep the host's typed `commandRejected`
  refusal. `session/fork` never carries the member.

- Adopt Muse 1.4.8 Computer Use app-level choices (#253). The opt-in
  `_session/computerUse` settings now expose effective `apps` rows and can
  request `installedApps` with `installed: true`; `/cua status` renders each
  app's name, effective choice, last-asked time, and lock reason.
  `/cua apps` lists the installed/running picker source, and `/cua app
  <bundleId> ask|alwaysAllow|blocked [...]` applies one atomic multi-app patch.
  Locked, duplicate, and overlapping targets preserve the host's typed
  `commandRejected` refusal.

- Adopt Muse 1.4.8 monitor feed items (#255). A `monitorFeed` item renders as
  one stable ACP tool card per `monitorId`; streamed `text` deltas accumulate
  into replacement card content and the completed item settles it. The
  model-controlled arm-time description is stripped of control and bidi
  formatting characters, collapsed and bounded for the card title, while
  `_meta.muse.monitorId` links the card to the monitor tool call. Feed items
  arriving outside a turn (`turnId: null`) are accepted, and host-truncated
  text is reported with `_meta.muse.truncated.source = "host"`.

- Adopt Muse 1.4.8 generated session short titles (#256). The adapter folds
  the optional `Session.generatedName` snapshot fact through start, resume,
  load, and fork, and accepts live replacements from
  `SessionNameChangedParams.generatedName`. Clients receive the host's value
  as `session_info_update._meta.muse.generatedName`, separate from the
  canonical ACP title. Missing values publish nothing and are never inferred
  from the name or title fallback; an authoritative withdrawal after
  publication sends null. `session/list` rows neither expose nor erase the
  fold-only fact.

- Adopt Muse 1.4.9 native approval-reviewing activity (#271). When the host
  reports `approval/reviewing`, the adapter publishes reversible
  `session_info_update._meta.muse.approvalReviewing` rows with source session,
  approval id, title, and tool kind. The activity never opens a permission
  prompt or starts adapter Auto-review. The row clears when
  `approval/requested` releases the decision, `approval/resolved` settles it,
  or the owning turn ends. Parent and child approvals with the same id remain
  distinct, while the adapter's own Auto-review continues to handle only a
  later released request.

- Adopt Muse 1.4.9 tool display labels (#272). Tool-card titles now prefer
  `Item.displayLabel.active` while a call runs, `.done` after success, and
  `.neutral` for every other status. Live snapshots, completed items, history
  replay, and AIR async-task announcements all use the same selected label.
  Missing, null, malformed, or empty labels retain the existing synthesized
  `tool: detail` title, so pre-1.4.9 hosts keep their current presentation.
  Fake-host tests pin the active/done/neutral selection; a live 1.4.10 probe
  confirms tools without a label keep the synthesized title.

- Reconcile declined turn retractions with the Muse 1.4.10 fold (#273). When
  `turn/retracted` reports `outcome: "declined"`, the submission stood even
  though the interrupt still stops the turn. The adapter now keeps the ACP
  prompt, active turn, retry/background-check hints, steering target, and
  second-Stop target alive until the ordinary `turn/completed` terminal, and
  emits one transcript note explaining the retained message. Accepted, absent,
  and unrecognized outcomes retain the immediate cancelled settlement. A
  terminal-first declined retract is logged and ignored, matching real Muse
  1.4.10's observed ordering after committed output.

- Adopt Muse 1.4.7 host-level computer use (#240). A client that opts into
  `_meta.muse.computerUse` now has the adapter request both
  `computerUseSetup` and `computerUseHostSetup`. `/cua host` reads the host's
  setup state and three macOS grants, and `/cua host check` asks the host to
  inspect those grants without prompting or opening System Settings. These
  ordinary prompt commands do not require the custom `_session/computerUse`
  extension; `/cua status` and `/cua on` remain session-scoped. The grants are
  independent and fail closed, sessionless setup-change notices are consumed,
  and a busy helper preserves the host's typed `commandRejected` (`-32030`)
  `runtime_busy` data.

- Adopt the Muse 1.4.7 `SessionNameOrigin` name provenance (#242). The
  adapter folds `Session.nameOrigin` and `SessionNameChangedParams.nameOrigin`
  with the name they describe and publishes a proven origin (`automatic` or
  `custom`) as `_meta.muse.nameOrigin` on the title's `session_info_update`,
  including origin-only changes and restores on attach. Absent origins and
  unrecognized future values read as unknown and stay unpublished, and
  `session/list` rows — which never carry the fold-only origin — neither
  gain one nor erase a known one for the same name.

- Re-pin the vendored Muse SDK conformance corpus to `ae46934` (2026-10-16,
  "remirror: 1.4.10"), which publishes the Muse 1.4.9 stable surface that
  Muse 1.4.10 carries unchanged. The additions over 1.4.8 are additive: the
  tool-call display label (`Item.displayLabel` with the `ToolDisplayLabel`
  active/done/neutral triple) and the `approval/reviewing` notification for
  an approval the host's native reviewer holds. The vendored bundle is
  byte-identical to what an installed Muse 1.4.10 exports with
  `muse schema generate-json-schema`. The re-pin itself changes no adapter
  behavior: native-review activity and `displayLabel` adoption land
  separately in #271 and #272. The re-pin is documentation-only: the schema corpus, the compatibility
  table (the new fingerprint classifies as degraded until a pinned 1.4.9 or
  1.4.10 build joins the live-host matrix), `PROVENANCE.md`, the ROADMAP
  reference snapshot, sections 1, 2, and 9, the event matrix, and the field
  matrix. Each new surface the adapter does not drive has a tracking issue,
  #271 through #273.

- Adopt the remaining Muse 1.4.x field-level and vocabulary additions (#224).
  A turn failure that carries the typed `TurnError.quota` detail now names the
  exhausted limit and the provider's reset wait in the transcript, a
  `mcpLinkError` failure names the MCP-link remedy, and the
  `turn/retryScheduled` and `turn/retracted` diagnostics record the 1.4.6
  `errorKind`/`httpStatus`/`system` and `outcome` fields. The `webSearch` item
  kind renders through the unknown-kind generic card path. Every remaining
  addition (`SessionStartParams.permissionProfile`, `TurnStartParams.userShell`,
  the `providerId` fields, and the newly required-but-nullable params) has an
  explicit disposition in the ROADMAP section 9 field matrix, which
  `tests/msp_140_field_matrix.rs` enforces against the vendored bundle.

- Request the Muse 1.4.8 `sessionRename` capability and stay in step with its
  registry (#260). The adapter asks for `sessionRename` at `initialize` and
  records the grant, so `/rename <name>` keeps driving `session/rename` on a
  host that grants it. Muse 1.4.3 through 1.4.7 predate the registry entry:
  they ignore the unknown name and still serve `session/rename`, so `/rename`
  keeps working there. From 1.4.8 on, a host that withholds the grant is
  answered with the typed `capabilityRequired` (`-32010`, naming
  `sessionRename`) instead of a generic failure or an ungranted call.

- Mark agent replies that Muse cut off mid-stream (#243). When the provider
  stream dies mid-reply and Muse 1.4.7+ commits the partial text with
  `Item.cutOff`, the adapter marks that message once with
  `_meta.muse.cutOff: true` and the host `itemId`, so editors can show the
  reply as incomplete instead of finished. A streamed reply gets the mark on
  an empty `agent_message_chunk` (naming its `messageId` on ACP v2); a resent
  or replayed reply carries it on its own text update, so a reload never
  reopens a delivered `agent_message`. The mark comes from whichever of
  `item/updated`, `item/completed`, a gap refill, or history replay first
  carries the flag; it is never inferred, repeated, or cleared. The field
  joins the ROADMAP section 9 field matrix, and a live loopback test covers it
  on Muse 1.4.7 and later.

- Show each skill's own name in the command catalog (#258). Every skill
  command in `available_commands_update` now carries `_meta.muse.title`: the
  name the skill declares in `SKILL.md` (`metadata.display-name`, surfaced by
  Muse 1.4.8+ as `SkillCatalogEntry.title`), or Muse's `displayName` when the
  skill declares none. The command name stays the selector and the
  description stays the skill's summary; a skill whose summary is empty is
  described by its name instead. The field joins the ROADMAP section 9 field
  matrix, and a live loopback test covers it on Muse 1.4.8 and later.

- Record the Muse 1.4.7 sandbox-availability facts from the `initialize`
  handshake (#241). The adapter logs
  `host-sandbox server=<host> availability=… reason=…` next to
  `host-features`, plus a warning when the host reports it cannot enforce its
  sandbox. An absent field (Muse before 1.4.7) or an unrecognized value reads
  as `unknown`. A reason is shown only when the host reports `unavailable` and
  sends one, and is never inferred. `--support` now runs one handshake with
  `muse serve` to include the same facts (`support-host-sandbox`), and
  `--selftest` prints an offline `unknown` row. No behavior is gated on the
  facts. Both fields join the ROADMAP section 9 field matrix.

- Show Muse 1.4.8 mid-turn model retries (#259). A `turn/modelRetryScheduled`
  notification no longer takes the unhandled-notification path: it leaves the
  ACP prompt open, logs the attempt facts like `turn/retryScheduled`, and,
  while the turn is running, sends
  `session_info_update._meta.muse.modelRetry` with `turnId`, `attempt`,
  `nextAttempt`, `maxAttempts`, `retryDelayMs`, and the optional `errorKind`,
  `system`, and `httpStatus`, so an editor can render a retry countdown. An
  unknown `errorKind` passes through verbatim. The turn's terminal withdraws
  the hint with `modelRetry: null` before the prompt settles. A live loopback
  test covers it on Muse 1.4.8 and later.

## 0.14.0

- Re-pin the vendored Muse SDK conformance corpus to `838d36a` (2026-10-14,
  "remirror: 1.4.8"), which publishes the Muse 1.4.8 stable surface. The new
  surface is additive over 1.4.7: Computer Use app-level choices and the
  installed-app picker (`ComputerUseSettings.apps`/`.installedApps`,
  `ComputerUseSettingsReadParams.installed`,
  `ComputerUseSettingsUpdateParams.apps`), the agent-message phase
  (`Item.messagePhase` with the `MessagePhase` enum), the `monitorFeed` item
  kind (`Item.monitorId`/`Item.monitorDescription`, with `Item.truncated` and
  `Item.turnId` widened to cover it), the generated short title
  (`Session.generatedName`, `SessionNameChangedParams.generatedName`), the
  resume permission-profile pick (`SessionResumeParams.permissionProfile`),
  the skill's declared `title` (`SkillCatalogEntry.title`), the
  `turn/modelRetryScheduled` notification, and the `sessionRename` capability.
  The 1.4.8 build pinned below joins the live-host matrix in the same release,
  so the fingerprint classifies as tested rather than degraded; an installed
  Muse 1.4.8 exports a schema bundle and manifest byte-identical to the
  vendored copy. The event matrix files the new
  `turn/modelRetryScheduled` notification as `Intentionally ignored` (it takes
  the unknown-notification diagnostic path and never settles a prompt). Each
  new surface the adapter does not drive has a tracking issue, #253 through
  #260.

- Stop a resumed session from reopening approvals that were already decided
  (#238). Muse's pending list can still include approvals that durable history
  resolved; the adapter showed them again, deciding them failed with
  `approval <id> is already resolved`, and the stale prompt blocked every
  follow-up prompt. A reattached session now reads its history once, when its
  first approval arrives, and drops any approval history already resolved,
  whether it comes from the pending list, a reissued request, or a live event.
  An approval resolved elsewhere while queued for Auto-review is no longer
  reviewed.

- Stop a subagent's approval from hiding or withdrawing its parent's (#252).
  Muse approval ids are only unique within one session, so a subagent and its
  parent can both ask for `ap-1`. The adapter matched approvals by id alone:
  once the subagent's `ap-1` was decided, the parent's `ap-1` with the same
  requirement was dropped as a duplicate and its turn waited forever, and the
  parent's `ap-1` resolving could withdraw the subagent's prompt. Approvals are
  now matched by their Muse session and id together.

- Fix MCP tool approvals from Muse 1.4.6+ showing as `Muse action` with a
  stray pending tool card (#239). These approvals carry no top-level
  `toolName` or `toolCallId`; the tool name is only on the subject. The
  permission is now titled from `subject.toolName` (or the subject's
  description when it has no name) and points at the tool card already open
  for the approval's item, or a unique id when there is none, instead of a
  shared `?` card that never settled.

- Surface the reason when a permission is denied without an explicit editor
  rejection. Auto-review denials, reviewer failures, unusable replies, and
  review timeouts now add a short transcript message such as `Permission
  denied: reviewer timed out after 90s.`; the reviewer's rationale is still
  what Muse receives as decision feedback. A failed, missing, or unusable
  editor permission reply is explained the same way, without sending the
  notice to the model as feedback. A `cancelled` reply follows the user's own
  stop, so it adds no message. The adapter log records the client outcome,
  selected option, and RPC error for every fallback deny. Previously these
  fail-closed paths left only Muse's generic "approval aborted" tool result
  visible in the editor.

- Pin the Muse 1.4.6, 1.4.7, and 1.4.8 builds in the live-host matrix. The
  matrix had stopped at 1.4.4, so three published releases were never run
  against the adapter: 1.4.6-R6000.1, 1.4.7-R6101.1, and 1.4.8-R6256.1 each
  now run the loopback suite on Linux x86-64 and the workspace-root flows on
  Windows x64, and 1.4.8 replaces 1.4.4 as the newest pin that also runs on
  macOS arm64. Every checksum, and every recorded fingerprint, is the value
  in that release's own published download manifest. The compatibility table
  moves all three surfaces to tested: 1.4.6's is the surface the `ea88c822`
  SDK pin published, 1.4.7's is the retired `a0c8611` pin's surface, and
  1.4.8's is byte-for-byte the vendored `838d36a` corpus, so the corpus is
  live-validated instead of degraded. Two host details only the newer pins
  meet:
  Muse omits the top-level `toolName` for MCP tool approvals and carries it
  only as `subject.toolName` (#239), and Muse 1.4.8 refuses a
  `workflow/resume` that renames the run (`workflow_name_conflict`), so the
  loopback provider now resumes without inventing a name. Local runs of the
  full suite pass 37 of 37 tests against all three builds.

- Tell the editor when a turn's answer is done but Muse's background reminder
  checks still hold it open (#247). Muse 1.4.6+ sends
  `turn/foregroundCompleted` right after the final content, then keeps the turn
  running while reminder agents (`verify-reminder`, `goal-reminder`, …) run,
  which can take a minute. The Muse SDK treats that notice as explicitly
  non-terminal, so the ACP prompt still settles at `turn/completed`. The
  adapter previously only logged it to stderr, so editors saw no frames at all
  during the wait. It now sends a `session_info_update` with
  `_meta.muse.foregroundCompleted` (`turnId`, `blockingAgents`) and one
  in-progress `Background checks: <agents>` card. The card settles and the hint
  clears (`foregroundCompleted: null`) just before the prompt result. A
  reminder that resumes work and fires again settles the old card and opens a
  new one. With `MUSE_SHOW_REMINDERS`, the per-agent cards replace the single
  card, and the hint is still sent.

## 0.13.1

- Fix Auto-review denying every approval with "auto-review returned no usable
  decision" after a session has run many reviews. The adapter ran every
  review as a new turn in one long-lived reviewer session, so the session's
  history grew with each review prompt. Once the reviewer turn failed (for
  example, the provider rejected the oversized history), the adapter ignored
  the `failed` terminal, parsed the empty reply, and denied with the generic
  unusable-decision message. The reviewer session was never reset on that
  path, so every later approval failed the same way, and Muse reported each
  one as "approval aborted". Each review now starts a fresh reviewer session,
  and the reviewer host is recycled every 32 reviews. A `failed` or
  `cancelled` reviewer turn now denies with the host's error (for example,
  `reviewer turn failed: contextWindowExceeded: prompt is too long`) and
  resets the reviewer host. An unparseable reply says whether it was empty,
  logs a short excerpt to stderr, and also resets the reviewer host.

## 0.13.0

- Re-pin the vendored Muse SDK conformance corpus to `a0c8611` (2026-10-13,
  "remirror: 1.4.7"), which publishes the Muse 1.4.7 stable surface. The
  corpus keeps its 51 golden transcripts and their recorded frames; 45
  manifests re-pin only their authoring `fingerprint` field, and the schema
  grows to 69 methods with 39 notifications unchanged. The schema adds
  `computerUseSetup/check`, the `computerUseHostSetup` grant with host-level
  `computerUseSetup/read` and `/start` (an omitted `sessionId` reads or
  starts the host's own setup), the handshake's optional `sandboxAvailability`
  and `sandboxUnavailableReason` facts, `SessionNameOrigin` on the session
  snapshot and name-change notification, and `Item.cutOff` for a partial
  agent message. No pinned 1.4.7 build has joined the live-host matrix yet,
  so the new manifest fingerprint still reports `kind=sdk-manifest ...
  status=degraded` in `--selftest` and startup (a visible warning, never
  fatal) until one does. A local live run against Muse 1.4.7-R6101.1
  negotiated the same fingerprint and passed 36 of 37 loopback tests; the
  remaining failure is the open MCP approval-title bug
  [#239](https://github.com/BrokkAi/muse-acp/issues/239). The new fields are
  tolerated by the fold with their own tracking issues
  ([#241](https://github.com/BrokkAi/muse-acp/issues/241)–[#243](https://github.com/BrokkAi/muse-acp/issues/243)),
  and the undispositioned `computerUseSetup/check` method is tracked in
  [#240](https://github.com/BrokkAi/muse-acp/issues/240).
- Support chunked video uploads. A `resource_link` (or embedded resource
  blob) whose media type is `video/mp4` or `video/quicktime` is read inside
  the approved roots (up to 64 MiB), uploaded to the session's video store
  through Muse's `media/upload` in 1 MiB raw chunks, and submitted to the
  turn as a `{"type":"video","uploadId":...}` part. Chunks stay within the
  host's frame budget, each carries a fresh command id so a retry is
  idempotent, and a failed chunk fails the prompt before any turn is
  submitted.
- Add the opt-in computer-use surface. A client that advertises
  `_meta.muse.computerUse` makes the adapter request the host's
  `computerUseSetup` capability, and the `_session/computerUse` extension then
  reads the session's setup state plus the computer-use settings (built-in
  blocks with `showBuiltIns`). `/cua [status]`, `/cua on`, `/cua set
  computerControl|screenshots ask|alwaysAllow`, and `/cua block|unblock
  <bundleId>` mirror the host TUI for setup and settings patches. The CUA
  gate's `computerUseUnavailable` (-32033) keeps its typed data, and
  `computerUseSetup/changed` is consumed as a diagnostic with no local state.
- Add plugin and marketplace management. The opt-in `_session/plugins`
  extension (muse `plugins` capability) reads the session's installed plugin
  rows through the host's `plugin/list`; `/plugin install <marketplace>
  <name>` (or `/plugin install <path> [default|user|project]`) installs, and
  `/plugin marketplace add <name> <source>`, `remove <name>`, and
  `update <name>` manage marketplaces. The host's four write refusals
  (`marketplaceNotFound`, `pluginNotFound`, `marketplaceAlreadyExists`,
  `pluginMutationsDisabled`) keep their registry codes and typed data, and
  `plugin/changed` / `plugin/marketplaceChanged` are consumed as diagnostics
  with no local cache to invalidate.
- Reconcile background tasks from the host's authoritative inventory. On
  resume and re-attach, a Muse 1.4.6+ host with AIR async tasks negotiated is
  asked once for `task/list`; rows the replayed history did not carry are
  announced as stoppable async tasks keyed by the host's own task id, so the
  item events that follow settle the same task instead of announcing a
  duplicate, and a stop targets the host id directly. `task/background`
  remains unsupported.
- Support Muse side chats. The opt-in `_session/sideChat` extension (muse
  `sideChat` capability) opens a side chat from a loaded session through the
  host's `session/sideChat` and returns the new session id with its
  `sideFrom` provenance; the side is routed to the source's host, inherits
  its mode, and attaches through the ordinary `session/resume`/`session/load`
  paths. `sideFrom` (and a fork's `forkedFrom`) now rides
  `session_info_update._meta.muse` when a session is attached, so the parent
  link survives reloads.
- Add skill activation and import commands. `/skill-activation <selector>
  on|userInvocableOnly|off [scope]` writes one skill's activation through the
  host's `skill/setActivation` (scope defaults to `user`; a refused selector
  keeps the host's typed error), and `/skill-import <claude|codex> [dry-run]
  [force]` imports foreign skills through `skill/import` and renders the
  host's receipt: the four counts plus the installed, quarantined, skipped,
  and failed ids. The command catalog refreshes through the host's existing
  `skill/changed` path; the adapter keeps no local activation cache.
- Add `/recap`: ask Muse for a while-you-were-away summary. The prompt stays
  open until the host's `recap/completed` settles it, the summary (with its
  optional title) arrives as agent text, and one recap is allowed per session
  at a time. Stop sends `recap/cancel` and settles the prompt immediately
  rather than waiting out the host's 45-second timeout; failed and timed-out
  recaps are reported as failures, never as success.
- Label hook runs and expose the session's hook inventory. `hookRun` items
  now render as `Hook <label>` cards carrying the hook key, event, durable
  outcome, and terminal reason, and the opt-in `_session/hooks` extension
  (muse `hooks` capability) returns the host's `hook/list` rows verbatim and
  names each card's handler origin and enabled switch.
- Pause and resume live Muse workflows from the editor. `/workflow pause
  [<runId>]` and `/workflow resume [<runId>]` map onto the host's
  `workflow/pause` and `workflow/resume`; a single live run may be implied,
  a bare `/workflow` lists the live runs with their pause state, and the
  workflow card shows `(paused)` while the host's pause fact is in force. A
  resumed launch arrives as a new card whose `_meta` names the run it
  continued, and its driven turn stays interruptible with Stop.
- Keep the answers a user already confirmed when a pending Muse question is
  stopped or a later question is declined. ACP forms are atomic, so
  multi-question prompts are now asked one question per form and each
  confirmed answer is held until the last one is ready; a stop (or a decline
  after answering) then settles the prompt through MSP `userInput/interrupt`
  carrying those answers, which stops the turn instead of discarding them. A
  question with no confirmed answer still stops through the ordinary turn
  interrupt and still declines through `userInput/cancel`.
- Verify the Windows verbatim `\\?\C:\...` workspace-root form against real
  Muse hosts and keep it covered. A Windows x64 run of the multi-root,
  mode-change, and load/re-attach flows passed against every pinned build
  (1.4.1, 1.4.2, 1.4.3, 1.4.4; each Windows binary checksum-verified against
  Muse's published manifest), so 1.4.1 and 1.4.2 accept the form and no
  version gating is needed. The `live-host` matrix now runs those flows on
  Windows x64 for every pinned build. The same run recorded Windows
  host-platform gaps outside the flows: `session/delete` answers
  `UnsupportedPlatform` on 1.4.1/1.4.2, and the bash shell and 1.4.1 fork
  tests do not pass on Windows.
- Render Muse todo plans on ACP v2 connections. The adapter previously
  emitted only the v1 `plan` frame, which v2 clients drop, so plans silently
  never appeared on a v2 session. v2 now receives the todo snapshot as a
  `plan_update` items checklist (plan id `muse-todo`) and a `plan_removed`
  when the list is cleared; v1 keeps the existing `plan` frame.
- Remember the last reasoning effort picked per model in
  `$XDG_STATE_HOME/muse-acp/reasoning-efforts.json` (Windows:
  `%LOCALAPPDATA%\muse-acp\reasoning-efforts.json`) and apply it to new
  sessions, so a new session starts at the last effort the way it already
  starts at the last model. Picking `Muse default` forgets the remembered
  tier; a tier the new session's model does not serve is left at the Muse
  default.

## 0.12.1

- Silence background reminder checks in the transcript. `reminderChild` items
  (`skill-reminder`, `goal-reminder`, `verify-reminder`, …) hold the turn open
  but carry no editor-facing content — the host shows nothing per agent — so
  they no longer render `Reminder from <agent>` / `Reminder child session`
  cards, keeping `in_progress` rows with weak detail out of the transcript.
  The blocking agent ids are still logged on stderr with
  `turn/foregroundCompleted`. Set `MUSE_SHOW_REMINDERS=1` (`true`/`yes`/`on`)
  to render the per-agent cards again.
- Re-pin the vendored Muse SDK conformance corpus to `ea88c822` (2026-10-11,
  "remirror: 1.4.6"), which publishes the Muse 1.4.6 stable surface. Unlike
  the previous `bb44be3` (1.4.2) and `537cc8d` (1.4.4) pins, this fingerprint
  has no live-validated host yet: `--selftest` and startup now report
  `kind=sdk-manifest ... status=degraded` (a visible warning, never fatal)
  instead of `tested`, and it joins the tested table once a pinned 1.4.6 build
  runs in the live-host matrix. The corpus grows to 39 notifications and 68
  methods; the four new notifications (`computerUseSetup/changed`,
  `plugin/changed`, `plugin/marketplaceChanged`, `recap/completed`) are
  classified in the event matrix and take the unhandled-notification
  diagnostic path.
- Record explicit dispositions for the Muse 1.4.3–1.4.6 surface the adapter
  does not drive — `plugin/list`, `plugin/install`, the `plugin/marketplace*`
  methods, `recap/request`, `recap/cancel`, `computerUseSettings/read`,
  `computerUseSettings/update`, `computerUseSetup/read`,
  `computerUseSetup/start`, `skill/setActivation`, `skill/import`,
  `workflow/pause`, `workflow/resume`, `task/list`, `hook/list`,
  `media/upload`, `session/sideChat`, `userInput/interrupt`, the
  `computerUseSetup` capability, and the four new plugin/marketplace error
  kinds — in the ROADMAP section 9 matrix, each linked to a tracking issue
  (#214–#224). Nothing is adopted in this release; the adapter keeps refusing
  or ignoring them exactly as before, now with the gap recorded.

## 0.12.0

- Fold reminder-child cards onto one card per reminder agent. The host mints a
  new `reminderChild` item for every background re-check and cancels the
  previous generation, so naming the generation in the title appended a
  transcript row per re-check, and every superseded generation settled as a
  failed card. The title is now the stable `Reminder from <agent>` (then
  `Reminder <task>`, then the server summary), the generation moves into the
  card body, and a new generation updates the same card. A superseded
  (`cancelled`) generation settles as completed instead of a failed card: the
  cancellation is host churn, ACP has no `cancelled` tool status, and ADR 41191
  D4 keeps the reminder's status word off the wire. A real
  `failed`/`rejected`/`timedOut` outcome still shows as failed, parallel
  reminder agents keep distinct cards, and a card announced at start still
  settles when its terminal frame carries no title (#1007).
- Refresh the README adapter comparison with October 2026 package metadata and
  relative npm adoption.

## 0.11.1

- Fix Auto-review against a real Muse host. The reviewer session started its
  turn with the pre-`input` `prompt` field, which Muse 1.4.x rejects with
  "invalid turn/start params: missing field `input`". Every approval then
  failed closed as `reviewer turn failed`, which Muse surfaced to the editor
  as "approval aborted", on every approval and in every client. The reviewer
  now submits `turn/start` with `input`, the shape the rest of the adapter
  already used and the MSP schema requires. The fake MSP host now rejects a
  `turn/start` without `input`, and the live-host suite covers the adapter's
  own reviewer against a real `muse serve`.
- Document that Auto-review requires Muse 1.4.2 or newer. On Muse 1.3.0 and
  1.4.1 the reviewer host's own background reminder checks hold the review
  turn open, so the review times out and denies; the live-host reviewer test
  now skips those pins instead of failing on a host-side limitation.
- Consume `turn/foregroundCompleted` instead of falling through to the
  unhandled-notification diagnostic. Muse's newer hosts emit it when a turn's
  foreground work is done but named background reminder checks still hold the
  turn open; it is explicitly non-terminal, so the ACP prompt stays open until
  `turn/completed` / `turn/unqueued`. The adapter now logs the turn, session,
  and blocking reminder-agent ids.

## 0.11.0

- Deny a hung auto-review past a deadline. A reviewer turn that never ends no
  longer stalls the approval indefinitely: after 90 seconds (Codex's guardian
  review timeout, overridable with `MUSE_REVIEW_TIMEOUT_MS`) the adapter logs
  the timeout, denies with a reviewer-unavailable rationale, resets the
  reviewer session, and starts the next queued review.

- Name reminder-child cards after their agent. A `reminderChild` item used to
  render as a generic card showing only "Reminder child session", so parallel
  reminders were indistinguishable. The card title now names the reminder
  agent and generation, falling back to the task id and then the server
  summary as before, with the task, child session, and log path in the card
  body.
- Support exact-turn steering on ACP v1. `_session/steering` now works on both
  protocol versions: the v1 `initialize` response advertises
  `steering.supported`, v1 connections are accepted instead of failing with
  `-32601`, and the synthetic user echo uses the v1 `user_message_chunk` shape
  without a `state_update`. Previously steering was reachable only from ACP v2
  clients, so ACP v1 clients such as Mjolnir could not steer a Muse turn.
- Fix Windows sessions with extra workspace roots. Muse 1.4.3 requires
  `turn/start workspaceRoots` entries in the verbatim `\\?\C:\...` canonical
  form, so the adapter keeps that form instead of stripping it back to
  `C:\...`, and `session/start` now names the same folder in `workspaceRoot`.
  Previously a session that re-attached after a mode change, resume, or fork
  could fail its next turn with "expected a canonical path".
- Classify Muse 1.4.4 as a tested host. A live `muse serve --provider echo`
  1.4.4 handshake reports the new stable fingerprint, and the binary's own
  `muse schema` export is identical to the vendored bundle, so startup logs
  `status=tested` instead of `status=unknown`. `session/delete` stays
  removed; the version-gated features are unchanged (no delete from 1.4.3
  on, workspace roots and host-computed cost stay on). The live-host suite
  now runs against 1.4.4 on Linux, and the newest macOS arm64 pin moves to
  1.4.4.
- Re-pin the vendored Muse SDK conformance corpus to `537cc8d`. It publishes
  the Muse 1.4.4 stable surface, adds the `userinput-interrupt-round-trip`
  transcript (51 scenarios), and drops `session/delete` and
  `session/deleteCompleted` from the schema; the event-compatibility matrix
  classifies the new `turn/foregroundCompleted` and `userInput/engaged`
  notifications as intentionally ignored.
- Run the live-host suite against Muse 1.4.3 (`1.4.3-R5018.1`) on Linux and
  macOS arm64, and classify its schema fingerprint as tested. On 1.4.3,
  Read-only and Plan sessions no longer get write or shell tools at all,
  instead of having the call denied; writes stay blocked either way.
- Stop offering session delete on Muse 1.4.3. That release removed
  `session/delete` and answers it with "method not found", so the adapter
  no longer advertises delete there and refuses the request itself, keeping
  the session. Delete still works on Muse 1.4.1 and 1.4.2.
- Publish a native Windows arm64 build (`aarch64-pc-windows-msvc`). Releases
  now carry its ZIP and checksum, the npm package bundles it for
  `win32/arm64`, and `install.ps1` installs it on arm64 machines, including
  from an emulated x64 PowerShell, which reports `AMD64` in its environment.
  Previously the npm launcher and the installer refused Windows arm64.

## 0.10.0

- Add session-scoped **Auto-review**. A new default-off `auto_review`
  selector sends every permission request to a reviewer agent instead of the
  editor. The reviewer runs on a memory-only, read-only Muse host, follows a
  Codex-style safety policy, and answers with a risk level, user-authorization
  score, allow/deny outcome, and rationale; failures deny. The selector never
  changes the host's approval mode, and each decision is logged with its
  rationale. See the README Auto-review section and
  [docs/auto-review.md](docs/auto-review.md).
- Add **Read-only** and **Plan** session modes. The editor's Mode selector
  now offers Default, Read-only, and Plan. Read-only and Plan sessions run on
  a second `muse serve` started with `--disable-write --disable-shell`, so
  Muse itself refuses file writes and shell commands. Plan also tells Muse to
  plan rather than implement, and a bare `/plan` switches to it without
  starting a turn. Changing an open session's mode moves it between the two
  hosts and is refused while a turn or background work runs on the host it
  leaves. The mode is remembered, so a reloaded session comes back in it.
- When the Muse host restarts, a session that had not run a turn yet starts
  again under its own id. Muse saves a session only with its first turn, so
  such a session previously failed to reconnect.
- The approval policy moves to its own **Approval Mode** selector
  (`approval_mode`). Approval mode ids sent to `mode` or `session/set_mode`
  still set it.
- When Muse declines `/compact`, for example on a session too short to
  compact, the prompt now ends normally with a note naming Muse's reason.
  Previously a `compaction_unavailable` or `missing_run` answer surfaced as an
  internal `-32603` error, and a no-op compaction ended without a word. On
  ACP v2 the session stays running if another turn still is.
- Classify Muse 1.4.2 as a tested host. Its stable schema only adds to
  1.4.1's, and the live-host suite passes against it, so startup logs now
  report `status=tested` instead of `status=unknown`.
- Re-pin the vendored Muse SDK conformance corpus to `bb44be3`. It publishes
  the Muse 1.4.2 stable surface, including `session/delete`, workspace roots,
  session-list filters, per-model reasoning tiers, cost totals, and feedback.
  Startup now logs a `host-features` line saying whether the host's version
  offers session delete, workspace roots, and host-computed session cost.
- ACP `session/delete` is advertised and backed by MSP `session/delete` on
  Muse 1.4.1+ hosts with durable session logs. The editor's answer arrives
  with the host's terminal event, a completed delete disappears from
  `session/list`, and a refusal keeps the session and explains why. Muse only
  deletes sessions it can prove the running host owns, so a session from an
  earlier editor run may be kept; the host's own reason and physical-change
  evidence are shown either way. Deleting a session that never existed
  succeeds silently where the host's listing filters can prove the absence
  (Muse 1.4.2+); on hosts without them the refusal is reported rather than
  pretended away.
- ACP `additionalDirectories` now reach Muse itself as MSP `workspaceRoots`
  on 1.4.1+ hosts, so Muse's own tools work in the extra folders. Roots are
  validated and canonicalized up front, duplicates collapse, and a load,
  resume, fork, or host re-attach replaces the host's sticky root set on the
  next turn. Older hosts keep adapter-side confinement and log the limit.
- The reasoning selector offers exactly the selected model's tiers, with
  Muse's descriptions, on hosts that publish per-model variants, and follows
  a model change; a model that cannot describe its tiers keeps the fixed
  list. `config_option_update` now carries the complete `configOptions` list
  as ACP requires, and host-reported model, reasoning, and approval changes
  refresh it. AIR clients see the current model's default when the host set
  no session-level recommendation.
- Session cost now comes from Muse's own `session/tokenUsage.cost` on 1.4.2+
  hosts, including whether it is partial, instead of the adapter's catalog
  estimate; hosts without it keep the labeled estimate. Host cache read and
  write totals ride `_meta.museCumulative`, and the estimate's provenance
  moves into `cost._meta` where ACP allows extensions.
- Add `/feedback`. On hosts that grant the feedback capability it sends
  `feedback/submit` and shows the host's receipt; with form elicitation, one
  form collects the classification, note, and explicit consent for local
  tracing and the session record, both defaulting off. Without forms the
  classified `/feedback <bug|bad|good|other> <note>` syntax submits directly.
- A session that Muse unloads, for example when its host shuts down or the
  session sits idle, now stays in the editor's session list. With Muse's live
  listing stream it used to disappear until it changed again, although Muse
  keeps it on disk and can reload it.
- With a saved `:auto-review` profile, files Muse creates while it runs, such
  as the credential from a first login or a first workspace trust, now land
  in your Muse configuration instead of the temporary settings view, where
  they were lost when it was removed. A login completed in a terminal while
  the agent runs now reaches it: right away, or on Windows without symbolic
  links at the next prompt, new session, or editor authentication.
- Fix startup on Windows for a saved `:auto-review` permission profile
  without Developer Mode or administrator rights. The private settings view
  now links folders with directory junctions and, when symbolic links are not
  allowed, files with hard links. A file Muse replaces through a hard link,
  such as a refreshed credential, is moved back over the original when the
  host exits, or at the next launch if the editor terminated the agent.
  Previously every request failed with `A required privilege is not held by
  the client (os error 1314)`.
- If the settings view cannot be built, start `muse serve` with the saved
  settings instead of failing every request, and name the cause in the error
  shown when Muse then refuses the profile. Another app's configuration entry
  that cannot be linked is now left out of the view instead of failing it.

## 0.9.0

- When Muse is not installed, report it as ACP's auth-required error with a
  clear "Muse Code is not installed" message, and name the terminal auth
  method **Set up Muse Code**. `muse-acp login` then offers to
  run Muse's official installer, asking first, before `muse login`.
  Previously requests failed with a generic `-32603` startup error.
- When `muse` is not on `PATH`, look for it where Muse's installer puts it
  (`MUSE_INSTALL_DIR`, `~/.local/bin`, or `%LOCALAPPDATA%\Programs\muse` on
  Windows). Editors often launch agents without that directory on `PATH`.

## 0.8.1

- Classify Muse 1.4.1 as a tested host. Its stable schema adds to the 1.3.0
  surface without removing or requiring anything, and the live-host smoke
  test passes, so startup logs now report `status=tested` instead of
  `status=unknown`.

## 0.8.0

- Forward the MCP servers an editor attaches to a session, such as Zed's
  context servers or the JetBrains IDE server, to Muse 1.3.0 and newer through
  typed session MCP configuration. Stdio and HTTP servers are supported, and
  ACP HTTP MCP support is advertised when the host grants `sessionMcp`. Tool
  calls go through Muse approvals. Servers are loaded as optional, so one that
  cannot start does not break the session. Loading a session re-sends the
  servers, and so does a host restart. SSE servers, malformed entries, and
  hosts without the grant are logged by server name only. Previously every
  client MCP server was dropped.
- Advertise an ACP terminal auth method, `muse-login`, so editors can offer
  login when Muse is not authenticated. It runs the new `muse-acp login`
  command, which hands the terminal to `muse login` (a browser device-code
  approval) and exits with its status. The adapter still never handles
  credentials.
- Complete the ACP handshake even when `muse serve` cannot start (for example,
  when Muse is not installed). Later requests return the startup diagnostic
  instead of the agent exiting, so the editor can show what to fix.
- Fail `session/new` and `session/load` with ACP's auth-required error when
  Muse has no credential, so editors such as Zed open their login screen
  before the first prompt. This reads Muse's experimental `account/read`,
  which reports only which kind of credential is in effect; the adapter now
  opts into the experimental MSP API for it. If the check is unavailable, the
  first prompt reports the error as before.
- On Windows, find the `muse.cmd` launcher that the Muse installer puts on
  `PATH`. Previously Muse was never found there unless `MUSE_CLI` was set.
- On Windows, `muse-acp install` records the full path to `muse-acp.exe` in
  Zed's settings, because the PowerShell installer does not change `PATH`.
- Make goal turns stoppable. A `/goal` command that starts a goal turn now
  keeps the editor prompt open until that turn ends, so the editor shows it
  running and Stop interrupts it (the host then pauses the goal). Stop and
  closing a session also reach a turn the host started on its own, such as a
  goal continuation.
- `/goal stop`, `/goal cancel`, and other one-word control words now fail with
  guidance instead of becoming a goal whose objective is that word.
- `/goal`, `/rename`, and `/workflow-child` accept @-mentions, which join the
  command as `[@name](uri)` links; images still fail the command. Previously
  a mention turned the command into an ordinary prompt.
- `/goal`, `/rename`, and `/workflow-child` now work while a tool permission
  prompt is open, so a goal can be paused or cleared while its turn waits.
- Fix `muse-acp install` and `uninstall` on Windows: they now edit
  `%APPDATA%\Zed\settings.json`, where Zed reads its settings, instead of
  `~/.config/zed/settings.json`.

## 0.7.0

- Add adapter slash commands for Muse host controls: `/goal` sets, edits,
  pauses, resumes, or clears the session goal; `/rename <name>` renames the
  session; and `/workflow-child skip|retry <childId>` controls one child of a
  running workflow using its current attempt. Workflow cards now show child
  ids, and a bare `/workflow-child` lists the children you can control.
- Stop relaunching a Muse host that crashes right after every restart: at
  most five automatic restarts are allowed in any ten minutes, with growing
  backoff, before the adapter stops with an explicit message.
- Re-pin the vendored Muse SDK conformance corpus to `a7c10c5`. Its manifest
  matches the live-validated 1.3.0-R3401.1 host surface, so it now classifies
  as tested. Every emitted host request is validated against the schema's
  method index.
- Document the negotiated protocol extensions in the README and reconcile the
  roadmap's Muse 1.3.0 disposition table with landed support.

## 0.6.2

- Rewrite the README for new users, covering quick start, editor setup,
  configuration, and diagnostics, and align the contributing, releasing, roadmap,
  security, and protocol guides with current adapter behavior. The
  refreshed README ships in the release archives and the npm package.

## 0.6.1

- Start Muse hosts with human approvals when the saved profile is
  `:auto-review`, avoiding the unavailable-reviewer session failure without
  changing the user's Muse settings or editor launcher.
- Exercise permission-profile startup, approvals, restart, and cleanup on
  Windows as well as Unix, and fix the stdout saturation tests that blocked
  macOS and Windows release builds.
- Distribute the native adapter through `@brokkai/muse-acp` on npm, including
  all five supported platform binaries.

## 0.6.0

- Stop overriding Muse's reasoning default: sessions start at "Muse
  default" and send no per-turn tier until the user picks one, and the
  option is offered only while no standing default is in force.
- Gate leading-slash prompts on the skill catalog: fork sessions read
  skill/list, a failed re-read forgets a stale catalog, `/skill <name>`
  and `compact` always submit, and slash text naming no skill is sent as
  ordinary text.
- Settle orphaned in-flight prompts across durable host restarts,
  including legacy sessions and sessions the restarted host cannot
  re-attach.
- Withdraw stale client requests with `$/cancel_request` for resolved
  child-stream approvals and questions settled elsewhere.
- Harden the transcript gap walk: refill from the hole, stop on cursor
  cycles, and discard live twins of delivered pages.

## 0.5.0

- Add Muse Code 1.3.0 compatibility, native skills, session reasoning defaults,
  subscription usage observations, and negotiated stored-output access.
- Improve session listing, pagination, workspace filtering, durable names,
  metadata, status, and attention updates.
- Restore transcript history, usage, and active tasks across resume and host
  restarts; prevent duplicate updates during snapshot and gap replay.
- Add native subagent controls, workflow and background-task cancellation,
  queued-prompt cancellation, and host-confirmed file-change reports.
- Improve approval rule previews, rejection feedback, question clarification,
  multiple-selection forms, and legacy model selection.
- Harden workspace resource confinement, host shutdown, pipe timeouts, tool
  failure reporting, and authentication diagnostics.
- Support native Windows resource paths and terminate Windows host process
  trees on timeout; fix Windows integration test paths and synchronization.
- Add a PowerShell installer and document platform support, the Muse 1.3.0
  event compatibility matrix, and experimental account authentication.

## 0.4.5

- Recover draft releases through authenticated listing and read uploaded assets
  back by release ID when GitHub hides drafts from the tag endpoint.

## 0.4.4

- Surface actionable Muse authentication failures in ACP sessions.
- Decode local Windows file URIs with drive letters correctly.
- Ignore local worktree and Brokk files.
- Validate all release platforms and actual Actions publisher permissions before
  publication; verify complete artifacts and safely resume partial uploads.
