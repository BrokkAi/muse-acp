# Let Muse run commands in ACP v1 editor terminals

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.

It builds on `.agents/plans/editor-fs-bridge.md` (issue #310), which added the adapter's own MCP server, `editor`. That server is served over HTTP on 127.0.0.1 from `src/bridge.rs` and authenticated by a per-session bearer token. It lets Muse's model call the ACP v1 client's `fs/*` methods. Read that plan for the transport, tokens, confinement, and card matching. This plan adds one tool to the same server.


## Purpose / Big Picture


An ACP v1 editor that sends `clientCapabilities.terminal: true` in `initialize` will run commands for the agent in its own terminals: `terminal/create` starts a command and returns a terminal id, and the agent can embed that id in a tool card (`{"type":"terminal","terminalId":...}`) so the user watches it. `terminal/wait_for_exit`, `terminal/output`, `terminal/kill`, and `terminal/release` follow it to the end. ACP v2 removed these methods, so this is ACP v1 only. GitHub issue #311 asks for it.

Muse runs its own `bash` tool and MSP cannot hand it to the client. After this change the model can call `mcp__editor__run_in_terminal`. The command then runs in an editor terminal, the tool card shows that terminal live, and the model gets the output and the exit status. Read-only and Plan sessions refuse it, and cancelling the turn kills the command.

To see it working, run `cargo test --locked --test acp_serve editor_terminal`, and with a real Muse, `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback the_editor_terminal`.


## Progress


- [x] (2026-10-20 19:52Z) Read Zed's `terminal/create` implementation to choose how the command line is passed (see Decision Log).
- [x] (2026-10-20 19:58Z) `run_in_terminal` in `src/bridge.rs`, with typed client errors and per-request waits, terminal card content, live embedding, and unit tests.
- [x] (2026-10-20 20:00Z) Fold support in `src/fold.rs`: running editor cards are tracked, a recorded terminal replaces the completed card's text, and MCP tools are classified by their own name.
- [x] (2026-10-20 20:02Z) Approval prompts for editor tools name the file or command (`bridge::approval_title`, `with_editor_approval_args`, `retitle_editor_permission` in `src/main.rs`).
- [x] (2026-10-20 20:03Z) Fake-host scenarios and four black-box tests; one live loopback test, passing on Muse 1.3.0, 1.4.8, and 1.4.11.
- [x] (2026-10-20 20:03Z) README, ROADMAP, and CHANGELOG.


## Surprises & Discoveries


- Observation: Zed does not run `command` with `args` directly. It builds one shell command line from them, quoting each argument when `args` is non-empty and using `command` verbatim when `args` is empty, and runs that in the user's shell. The ACP spec instead describes `command` and `args` as a program and its arguments. Sending an explicit shell with the line as its last argument, such as `bash` `["-c", line]`, works under both readings.
  Evidence: Zed `crates/acp_thread/src/acp_thread.rs` `create_terminal` calls `ShellBuilder::build(Some(command), &args)`, and `crates/util/src/shell_builder.rs` `build` quotes each argument with `try_quote`.

- Observation: MCP approvals often reach the adapter without the call's arguments. On Muse 1.4.11 the prompt opens from `approval/updated` (review completed), which has no `rawArgs`, after an `approval/reviewing` that had them. On 1.4.8 it opens from `approval/updated` (review started) before `approval/request` and `approval/requested` bring `rawArgs`. While a native review runs, `approval/listPending` does not list the approval. Muse 1.3.0 sends `rawArgs` on the opening event.
  Evidence: raw MSP captured with a `tee` wrapper as `MUSE_CLI`: on 1.4.8, `approval/updated {change: reviewStarted, rawArgs: null}` arrives first, then `approval/request {rawArgs: "{\"command\": \"echo hi\"}"}`.

- Observation: the card kind was `edit` for every `mcp__editor__*` tool, because `fold::tool_kind` matched "edit" inside the server name.


## Decision Log


- Decision: run the line as `bash -c <line>` on Unix and `powershell.exe -NoProfile -NonInteractive -Command <line>` on Windows, and name the shell in the tool description.
  Rationale: this works for clients that exec `command` with `args` and for Zed, which wraps and quotes them. The model writes bash for Muse's own `bash` tool, so bash keeps its habits. The description tells it the Windows shell.
  Date/Author: 2026-10-20 / Claude.

- Decision: a non-zero exit code is a normal result (`isError: false`). A timeout, a cancel, or a lost terminal is an error result. Every result includes the output so far and an exit line (`[exit code N]`, `[terminated by signal S]`, or `[stopped: ...]`).
  Rationale: a failing build is information for the model, not a tool failure. A stopped command did not finish, which is a failure.
  Date/Author: 2026-10-20 / Claude.

- Decision: wait for the exit with the call's own timeout (default 120 s, at most 1800 s). A cancel can stop that wait, but kill, output, and release run with a short timeout that a cancel cannot stop.
  Rationale: the clean-up must happen after the cancel that triggered it. ACP requires `terminal/release`.
  Date/Author: 2026-10-20 / Claude.

- Decision: send the terminal to the running card as soon as it exists, and attach it again to the completed card in place of the output text. The running card is found by Muse's `_meta.callId` (1.4.11), or else by tool and `command` among the fold's running editor calls (`SessionFold::editor_card`). The bridge waits up to 3 s for the card's announcement and skips the live update if the card never appears.
  Rationale: ACP v1 `content` replaces the card's content, so the completed update must carry the terminal again. An update must never name a card the client has not seen.
  Date/Author: 2026-10-20 / Claude.

- Decision: approval prompts for editor tools name the file or the full command (plus `cwd`). The arguments come from the approval's `rawArgs`, else from the native-review row that saw them, else from `approval/listPending`. An open prompt whose arguments arrive later is retitled with a `tool_call_update` on its card instead of being reopened.
  Rationale: the command runs outside Muse's sandbox, so the user must see it before approving. Reopening a prompt the user may be reading would be worse than a title change.
  Date/Author: 2026-10-20 / Claude.

- Decision: classify an MCP tool's card kind by its tool name, the part after `mcp__<server>__`.
  Rationale: server names such as `editor` or `readwise` matched the read and edit patterns.
  Date/Author: 2026-10-20 / Claude.


## Outcomes & Retrospective


ACP v1 editors that advertise `terminal` now run Muse's `run_in_terminal` calls in their own terminals, on Muse 1.3.0 and newer, which closes #311. The live test on three Muse releases shows the approval naming the command, the `terminal/*` sequence, the terminal embedded on the card, and the output and exit code reaching the model. Muse's own `bash` tool remains, and the model chooses between the two from their descriptions. Not done: streaming partial output back to the model while the command runs (the tool returns once the command exits), and user-shell (`_session/userShell`) commands, which still run in Muse.


## Context and Orientation


The tool lives in `src/bridge.rs` next to the file tools (`run_tool` dispatches `run_in_terminal` to `terminal_tool`). The production client closure in `call` sends each request with `client_request`, which takes a `Wait`: a timeout, and whether a cancel can stop the wait. It returns a `ClientError` (`Cancelled`, `TimedOut`, or `Failed(message)`). `embed_in_card` sends the live card update. `CardContent` (`Diff` or `Terminal`) is what a call leaves for its completed card. `take_card_content` hands it to the `item/completed` handler in `src/main.rs`, which puts a terminal in `SessionFold::editor_terminals`, and `on_item_completed` in `src/fold.rs` shows that in place of the text. `SessionFold::editor_calls` lists the announced, running editor calls. `src/main.rs` `open_approval` enriches editor-tool approvals with `with_editor_approval_args` and retitles a shown prompt with `retitle_editor_permission`. `acp::PendingPerm` records the prompt's card id and title, and `acp::ReviewingApproval` keeps `raw_args`.


## Plan of Work


The work is done; see Progress. To extend it, keep new tools behind their ACP capability in `bridge::negotiate` and `tool_names`. Use `allowed` for the mode check and `resolve_path` for any path or working directory.


## Concrete Steps


    cargo test --locked --bin muse-acp bridge::
    cargo test --locked --test acp_serve -- editor_terminal cancelling_the_turn_kills an_editor_terminal_approval
    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback the_editor_
    env MUSE_ACP_LOOPBACK=1 MUSE_CLI=/tmp/muse-bins/1.4.8-R6256.1/muse cargo test --locked --test live_loopback the_editor_


## Validation and Acceptance


With an ACP v1 client that advertises `terminal`, a model call to `mcp__editor__run_in_terminal` with `{"command":"cargo --version"}` first shows a permission titled `mcp__editor__run_in_terminal: cargo --version`. After approval the client receives `terminal/create` with `command: "bash"`, `args: ["-c", "cargo --version"]`, and the session's `cwd`, followed by a `tool_call_update` embedding the returned terminal id. Then come `terminal/wait_for_exit`, `terminal/output`, and `terminal/release`. The model receives the output followed by `[exit code 0]`. A `session/cancel` during the wait produces `$/cancel_request` for the wait and `terminal/kill`, and the result ends with `[stopped: the user cancelled the turn]`.


## Idempotence and Recovery


Additive. `MUSE_EDITOR_TOOLS=0` turns off every editor tool, the terminal included.


## Artifacts and Notes


Live (Muse 1.4.8): the prompt opened as `mcp__editor__run_in_terminal`, and within a moment a `tool_call_update` on the same card id retitled it `mcp__editor__run_in_terminal: echo hi && ls`. The tool card's kind was `execute`, and both its running and completed updates carried `{"type":"terminal","terminalId":"t1"}`.


## Interfaces and Dependencies


In `src/bridge.rs`: `pub enum CardContent { Diff(String), Terminal(String) }`, `pub fn take_card_content(acp_sid: &str, item: &J) -> Option<CardContent>`, `pub fn approval_title(tool_name: &str, raw_args: Option<&str>) -> Option<String>`. In `src/fold.rs`: `pub fn editor_card(&self, call_id: Option<&str>, tool: &str, key: &str, value: &str) -> Option<String>` and `pub editor_terminals: HashMap<String, String>`.
