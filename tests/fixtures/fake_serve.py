#!/usr/bin/env python3
"""Fake `muse serve` MSP host for the committed ACP integration test.

Speaks just enough MSP JSON-RPC (line-delimited, over stdin/stdout) for the
adapter handshake, then plays one scripted scenario per run. FAKE_SCENARIO
selects the script; FAKE_MODE sets the folded host approval mode
(default promptUnmatched); FAKE_LOG (optional) records received methods.

Scenarios (TURN_N = incrementing turn id per turn/start):
  happy        agentMessage completion, then turn/completed(completed)
  failed       no message, then turn/completed(failed)
  tool         toolCall completion with result text, then completed
  file_changes successful native file tools, including a replayed completion
  file_changes_ambiguous a shell tool whose writes cannot be inferred safely
  approval     approval/requested notification (two choices), then completed
  approval_req approval/request server-initiated REQUEST (no notification)
  approval_hang stays pending after the approval so client decisions can be tested
  approval_resolved_elsewhere approval/requested, then approval/resolved by policy
  child_approval_resolved a child-stream approval, then approval/resolved on the child
  question_settled_elsewhere userInput/requested, then userInput/settled by another actor
  questions    userInput/requested with options, then completed
  questions_multiple userInput/requested with multiple-selection options
  questions_resume reissue a pending question after both attach and usage backfill
  queued       1st turn/start: silence; 2nd: completed(TURN_1), completed(TURN_2)
  cancel_request 1st turn runs; later turns queue until one is reclaimed
  unqueued     turn/unqueued for the turn (never runs)
  retracted    turn/retracted for the turn (no completion follows)
  retract_then_completed retract, then a late turn/completed (settle once)
  retry_then_completed turn/retryScheduled, then a normal completion
  deferred_launch_error queued admission followed by a launchError terminal
  quiet        turn/start answers only; nothing follows (for close/cancel)
  subagent_control live native child; FAKE_SUBAGENT_CONTROL_STATUS selects its state
  subagent_child_approval child approval without displayable choices
  host_exit_classified exit after a turn/start ack with FAKE_HOST_EXIT_CODE
  host_exit_before_ack exit before a turn/start admission response
  host_exit_relaunch_unavailable first exit is retryable, replacement exits 5
  host_crash_loop every generation crashes (exit 1) right after it is usable
  support_exit stdin-free serve probe writes stderr and exits with a code
  workflow_control live workflow; workflow/cancel emits the later item/turn views
  skills_changed skill/list changes after a skill/changed notification
               (FAKE_SKILL_REFRESH_FAILS=1: the re-read has no skills array)
  skill_not_found skill/list still lists `stale`; turn/start rejects it
  async_task_stop       background task then a targeted task/stop terminal
  async_task_stop_all   background tasks then task/stopAll terminals
  load         session/resume serves inline history (for session/load replay)
  resume_active session/resume reports a running turn (for steering reattach)
  async_resume session/resume reports running background work
  catalog_grows model/list expands after the first snapshot
  catalog_refresh_failure valid catalog, malformed response, RPC error, empty catalog
  usage_turn   two model legs from two models plus a replayed leg, one turn
  usage_turn_cancelled one model leg, then a cancelled terminal
  usage_gap    view/gap refill page overlapping a live completion
  gap_after_next view/gap flushed after `next` (real host order); two-page refill
  gap_ephemeral_next the gap's `next` is an item/delta, so paging runs past it
  gap_cursor_cycle view/page answers with a nextCursor loop
  usage_rates_dropped priced catalog, then a refresh whose model has no cost
  usage_rates_empty   ... then a refresh returning models: []
  usage_rates_invalid ... then a refresh whose rates do not parse
  usage_rates_failure ... then a FAILED refresh (rates must survive)
  usage_resume session/resume serves an anchoredSnapshot carrying usage
  reasoning_resume session/resume restores a standing reasoning default
  reasoning_legacy rejects the 1.3.0 session-default method
  usage_snapshot_null snapshot whose contextUsage is null (the common shape)
  usage_inline inline by default; the explicit snapshot rung carries usage
  usage_inline_nosnapshot every rung downgrades; only the durable page has
               totals, and contextUsage is never durable (as on the real host)
  session_list_stream grants sessionListStream and emits row replace events,
               then a schema-shaped session/closed unload after session/list
  session_list_stream_denied sends the notification without granting the capability
  session_list_pagination returns a second page when its cursor is forwarded
  status_flags   session/statusChanged status, attention, open-enum, and null
                 viewCursor handling
  session_metadata host-authored title candidates, branch/attention metadata,
                   and a live session/nameChanged notification
  session_list_workspace_filter return no host sessions for an unmatched root
  session_list_pagination two-page session/list response keyed by its cursor
  view_subscribe_gap session/resume requires explicit cursor replay; the host
               sends the replayed item twice to verify adapter deduplication
  view_health     emits an unavailable live-view health notification
  subscription_usage  usage/read returns the host snapshot and usage/changed
                      sends a changed subscription observation
  subscription_first_observation  usage/read is absent before the first
                                  usage/changed notification
  rename_live  session/nameChanged updates a connected session
  rename_resume Session.name changes between start and resume
  rename_snapshot SnapshotState.name is the only name on resume
  rename_clear  resume reports a null name and must clear a stale title
  rename_list   session/list exposes the authoritative session name
  tool_stored_output tool result with outputRef/patchRef metadata
  tool_output_unavailable outputRef exists but item/readOutput fails
  close_stdin  closes the host's stdin after initialization, then exits
  stdout_close_stays_alive closes stdout but keeps the child alive briefly
  stderr_flood writes enough stderr to require a concurrent drain
  goal_wake    an idle goal/set|edit|resume wakes a goal turn that answers
               and completes (the ack names the fresh turn)
  goal_wake_hang the woken goal turn runs until turn/interrupt, which pauses
               the goal; a later wake verb names it as the busy turn
  goal_continuation the woken goal turn completes, then the host starts its
               own follow-up turn that runs until turn/interrupt
  mcp_oauth_completed an experimental mcpServer/oauthLoginCompleted from
               another client's login flow follows the session/start ack

Host identity (every scenario): FAKE_SERVER_VERSION sets serverInfo.version
(default 0.0.0-fixture, which no version gate accepts), and
FAKE_SESSION_DURABILITY sets sessionDurability (absent by default, which MSP
reads as durable).

Session delete: `session/delete` acks, then reports `session/deleteCompleted`.
FAKE_DELETE_OUTCOME=failed sends a failed terminal with FAKE_DELETE_REASON
(default ownershipUnavailable) and FAKE_DELETE_PHYSICAL (default none);
FAKE_DELETE_REJECT=session_deleted|runtime_busy|sessionNotFound rejects at
admission; FAKE_DELETE_DELAY_MS delays the terminal; FAKE_DELETE_EXIT=1 exits
after the ack instead of reporting. A completed delete hides the id from later
session/list results. FAKE_LIST_NULL_ROOT=1 adds a row without a workspace
root; FAKE_LIST_REJECT_CURSOR=1 rejects a paged session/list with invalid
params.

Feedback: `feedback/submit` is granted when `feedback` is requested unless
FAKE_NO_FEEDBACK=1 (which makes the method fail with capabilityRequired).
FAKE_FEEDBACK_OUTCOME selects the result outcome (default uploaded);
FAKE_FEEDBACK_NOTES=1 adds sessionNote and localTracingNote;
FAKE_FEEDBACK_HOST_ERROR=1 fails the method with an internal error.

Session MCP (every scenario): `sessionMcp` is granted when requested unless
FAKE_NO_SESSION_MCP=1. As on the live host, a non-empty config.mcpServers
without the grant fails with capabilityRequired. FAKE_MCP_CONFLICT=1 rejects
the first session/resume that carries a non-empty config.mcpServers with the
live `session_configuration_conflict` error.
"""
import json
import os
import sys
import time

FP = "sha256:03312c213efd14277a0e0a102f70adeae497a469ca4edf7242f479953ed758b7"
SCHEMA = {"fingerprint": FP, "version": 1}
MSP_SID = os.environ.get("FAKE_SESSION_ID", "msp-sess-1")
SCENARIO = os.environ.get("FAKE_SCENARIO", "happy")
MODE = os.environ.get("FAKE_MODE", "promptUnmatched")
# The adapter launches a third, memory-only host for auto-review. It serves
# one reviewer session and answers each review turn with scripted JSON.
REVIEW_HOST = "--no-session-log" in sys.argv
REVIEW_SID = "msp-reviewer"
# When set, session/setApprovalMode folds to this mode instead of echoing the
# request, modelling a host that downgrades or pins the effective mode.
FOLDED_MODE = os.environ.get("FAKE_FOLDED_MODE", "")
LOG = os.environ.get("FAKE_LOG", "")
TURNS = [0]
CATALOG_READS = [0]
# The adapter's MSP initialize posture determines whether the host should
# create a user-input request. An explicit override exercises the backstop for
# hosts that predate userInputDialogs or ignore it.
USER_INPUT_DIALOGS = [True]
EXPERIMENTAL_API = [False]
SESSION_MCP = [False]
FEEDBACK = [False]
MCP_CONFLICTED = [False]
USAGE_READS = [0]
SKILL_READS = [0]
FORK_ITEMS = []
# Durable session ids a successful `session/delete` removed.
DELETED_SESSIONS = []

# Compatibility-diagnostics knobs: the fixture defaults to the validated
# host shape, but tests can present an unknown fingerprint or a future
# envelope schema version.
if os.environ.get("FAKE_FINGERPRINT", ""):
    SCHEMA["fingerprint"] = os.environ["FAKE_FINGERPRINT"]
if os.environ.get("FAKE_SCHEMA_VERSION", ""):
    SCHEMA["version"] = int(os.environ["FAKE_SCHEMA_VERSION"])

APPROVAL_PARAMS = {
    "sessionId": MSP_SID, "approvalId": "ap-1", "toolCallId": "call-1",
    "toolName": "workspace-shell",
    "subject": {"kind": "shell", "command": "cargo test"},
    "availableChoices": [
        {"choiceId": "c-allow", "label": "Allow",
         "decision": "approved", "scope": "once"},
        {"choiceId": "c-deny", "label": "Deny",
         "decision": "denied", "scope": "once"},
    ],
}
if os.environ.get("FAKE_APPROVAL_SUBJECT", "") == "file-write":
    APPROVAL_PARAMS["toolName"] = "workspace-files"
    APPROVAL_PARAMS["subject"] = {
        "kind": "fileAccess", "access": "write", "path": "/tmp/output.txt",
    }
if os.environ.get("FAKE_APPROVAL", "") == "all-approve":
    APPROVAL_PARAMS["availableChoices"] = [
        {"choiceId": "c-yes", "label": "Yes",
         "decision": "approved", "scope": "once"},
        {"choiceId": "c-always", "label": "Always",
         "decision": "approved", "scope": "session"},
    ]
if os.environ.get("FAKE_APPROVAL_CHOICES", "") == "empty":
    APPROVAL_PARAMS["availableChoices"] = []
if os.environ.get("FAKE_APPROVAL_FEEDBACK", "") == "deny":
    for choice in APPROVAL_PARAMS["availableChoices"]:
        if choice.get("decision") == "denied":
            choice["acceptsFeedback"] = True


# A host launched read-only logs its methods as `ro:<method>`, so tests can
# tell which of the adapter's two hosts handled a request.
READ_ONLY = "--disable-write" in sys.argv and "--disable-shell" in sys.argv


def log_method(method):
    if LOG:
        with open(LOG, "a") as f:
            f.write(("ro:" if READ_ONLY else "") + method + "\n")


def log_input(params):
    path = os.environ.get("FAKE_INPUT", "")
    if path:
        with open(path, "a") as f:
            f.write(json.dumps(params) + "\n")


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def validate_workspace_roots(roots, primary):
    """The live host's workspaceRoots rules, so tests catch bad frames."""
    if not isinstance(roots, list) or not roots:
        return ("workspaceRoots must be non-empty (omit the field for "
                "single-root behavior)")
    if not all(isinstance(root, str) and os.path.isabs(root) for root in roots):
        return "workspaceRoots entries must be absolute paths"
    seen = []
    for root in roots:
        canonical = os.path.realpath(root)
        if canonical in seen:
            return "duplicate root " + root
        if not os.path.isdir(root):
            return "not an existing directory: " + root
        seen.append(canonical)
    if primary and os.path.realpath(primary) != seen[0]:
        return "workspaceRoots[0] must name the same folder as workspaceRoot"
    return None


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def gap_message(tid, label, cursor):
    return {"sessionId": MSP_SID, "turnId": tid, "viewCursor": cursor,
            "item": {"itemId": f"gap-{label}", "kind": "agentMessage",
                     "turnId": tid, "status": "completed",
                     "text": f"gap event {label}"}}


def turn_id():
    TURNS[0] += 1
    return f"turn-{TURNS[0]}"


# The session notifications currently target; a fork switches it so turns
# started on the forked session complete on the forked session.
ACTIVE_SESSION = [MSP_SID]
CRASH_AFTER_ACK = [False]
WORKFLOW_TURN = [""]
WORKFLOW_CHILD_ATTEMPT = [1]
# The goal turn currently running in the goal_* scenarios ("" when idle).
GOAL_TURN = [""]
GOAL_OBJECTIVE = [""]


def goal_changed(status):
    notify("session/goalChanged", {
        "sessionId": MSP_SID, "viewCursor": f"cur-goal-{TURNS[0]}-{status}",
        "sourceRange": {"start": 1, "end": 1},
        "goal": {"objective": GOAL_OBJECTIVE[0], "status": status,
                 "percentComplete": 0}})


def on_goal_command(method, params):
    """goal/* ack. In the goal_* scenarios a wake verb on an idle session
    launches a goal turn and names it; on a busy session it names the turn
    already running, like the real host's routing fact."""
    result = {"commandId": params.get("commandId", ""), "status": "accepted"}
    if params.get("objective"):
        GOAL_OBJECTIVE[0] = params["objective"]
    if (SCENARIO not in ("goal_wake", "goal_wake_hang", "goal_continuation")
            or method not in ("goal/set", "goal/edit", "goal/resume")):
        return result
    if GOAL_TURN[0]:
        result["turnId"] = GOAL_TURN[0]
        goal_changed("active")
        return result
    tid = turn_id()
    base = {"sessionId": MSP_SID, "turnId": tid}
    result["turnId"] = tid
    goal_changed("active")
    notify("turn/started", {**base, "commandId": params.get("commandId", "")})
    notify("item/completed", {**base, "item": {
        "itemId": f"it-goal-{tid}", "kind": "agentMessage", "turnId": tid,
        "status": "completed", "text": "working on the goal"}})
    if SCENARIO == "goal_wake_hang":
        GOAL_TURN[0] = tid
        return result
    notify("turn/completed", {**base, "terminal": "completed"})
    if SCENARIO == "goal_continuation":
        # The host's own follow-up: no client command, no prompt owns it.
        follow = turn_id()
        GOAL_TURN[0] = follow
        notify("turn/started", {"sessionId": MSP_SID, "turnId": follow,
                                "commandId": "runtime-goal-" + follow})
    return result


ACTIVE_WORKSPACE = [os.environ.get("FAKE_WORKSPACE_ROOT", "/tmp/fake-ws")]


def approval_params(**overrides):
    """Approval params with the file-write path resolved at emission time.

    FAKE_APPROVAL_PATH=workspace points a fileAccess subject at the active
    session workspace; any other non-empty value is used literally. The
    default subject (shell) is returned unchanged.
    """
    params = dict(APPROVAL_PARAMS)
    params.update(overrides)
    subject = params.get("subject")
    if isinstance(subject, dict) and subject.get("kind") == "fileAccess":
        override = os.environ.get("FAKE_APPROVAL_PATH", "")
        if override == "workspace":
            subject = dict(subject)
            subject["path"] = os.path.join(ACTIVE_WORKSPACE[0], "output.txt")
            params["subject"] = subject
        elif override:
            subject = dict(subject)
            subject["path"] = override
            params["subject"] = subject
    return params


def session_obj(session_id=None, workspace_root=None):
    if session_id is None:
        session_id = ACTIVE_SESSION[0]
    if workspace_root is None:
        workspace_root = ACTIVE_WORKSPACE[0]
    session = {"sessionId": session_id, "modelId": "fake-model",
               "workspaceRoot": workspace_root,
               "activeTurnId": "turn-resumed" if SCENARIO == "resume_active" else None,
               "approvalMode": {"lastCommandId": None, "mode": MODE,
                                "source": "serverDefault"}}
    if SCENARIO == "status_flags":
        session.update({"status": "running",
                        "attention": ["approvalPending", "futureAttention"]})
    if SCENARIO == "session_metadata":
        if session_id == MSP_SID:
            session.update({
                "name": "Host session name",
                "title": "Host session title",
                "firstUserPrompt": "Host first prompt",
                "branch": {"branch": "feat/metadata", "vcs": "git",
                            "workspaceRoot": workspace_root},
                "attention": "needs-review",
            })
        elif session_id == "msp-sess-old":
            session.update({
                "title": "Host title fallback",
                "firstUserPrompt": "Host old first prompt",
            })
        elif session_id == "msp-sess-untitled":
            session["firstUserPrompt"] = "Host first prompt fallback"
    if SCENARIO == "rename_list":
        session["name"] = "Listed name"
    if SCENARIO == "session_list_stream" and session_id == MSP_SID:
        session["title"] = "Initial title"
    return session


def history_items():
    return [
        {"itemId": "h-1", "kind": "userMessage", "text": "old question"},
        {"itemId": "h-2", "kind": "agentMessage", "text": "old answer"},
        {"itemId": "h-3", "kind": "toolCall", "callId": "h-call",
         "status": "completed", "tool": "read",
         "args": {"path": "/tmp/h"}, "result": "old bytes"},
    ]


def cumulative_totals(prompt, output):
    """A session cumulative block, with the optional 1.4.2 cache and cost."""
    cumulative = {"promptTokens": prompt, "outputTokens": output,
                  "totalTokens": prompt + output}
    if os.environ.get("FAKE_CUMULATIVE_CACHE") == "1":
        cumulative["cacheReadTokens"] = min(prompt, 100)
        cumulative["cacheWriteTokens"] = min(prompt, 25)
    if os.environ.get("FAKE_CUMULATIVE_COST") == "1":
        cumulative["cost"] = {
            "usd": float(os.environ.get("FAKE_CUMULATIVE_USD", "0.25")),
            "partial": os.environ.get("FAKE_CUMULATIVE_PARTIAL") == "1",
        }
    return cumulative


def token_usage(cursor, prompt, output, cumulative_prompt, cumulative_output):
    """One `session/tokenUsage` completion leg, identified by view cursor."""
    return {"sessionId": MSP_SID, "turnId": f"turn-{TURNS[0]}",
            "promptTokens": prompt, "totalTokens": prompt + output,
            "modelId": "fake-model",
            "usage": {"inputTokens": prompt, "outputTokens": output,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": cursor,
            "sourceRange": {"start": 0, "end": int(cursor.split("-")[1])},
            "cumulative": cumulative_totals(cumulative_prompt, cumulative_output)}


def context_usage(used, cursor):
    return {"sessionId": MSP_SID, "usedTokens": used, "windowTokens": 200000,
            "pressure": "normal", "viewCursor": cursor,
            "sourceRange": {"start": 0, "end": int(cursor.split("-")[1])}}


def subscription_usage(window_percent, weekly_percent, observed_at=1754590990000):
    return {
        "observedAtMs": observed_at,
        "tier": "pro",
        "window": {"resetsAtMs": 1754608990000,
                    "usedPercent": window_percent,
                    "windowDurationMins": 300},
        "weekly": {"resetsAtMs": 1755200000000,
                    "usedPercent": weekly_percent},
    }


def usage_snapshot_history(context=True, cumulative=(100, 20)):
    """anchoredSnapshot history whose state already knows the usage. MSP
    serves `contextUsage` as null until the fold holds a tracked anchor, so
    context=False is the ordinary shape, not an exotic one."""
    return {"mode": "anchoredSnapshot", "items": None, "snapshot": {
        "schemaVersion": 1, "viewCursor": "cur-9",
        "anchor": {"boundaryCursor": "cur-8", "summarizedThrough": "anchor-8"},
        "state": {"items": [
                    {"itemId": "history-u", "kind": "userMessage",
                     "revision": 1, "status": "completed",
                     "text": "snapshot question"},
                    {"itemId": "history-a", "kind": "agentMessage",
                     "revision": 1, "status": "completed",
                     "text": "snapshot answer"},
                ], "activeTurn": None, "queuedTurns": [],
                  "pendingApprovals": [], "pendingUserInputs": [],
                  "approvalMode": session_obj()["approvalMode"],
                  "effectiveModel": None, "branch": None, "goal": None,
                  "todoList": None, "turnCount": 1,
                  "contextUsage": ({"usedTokens": 120,
                                    "windowTokens": 200000,
                                    "pressure": "normal"} if context else None),
                  "tokenUsage": {"promptTokens": cumulative[0],
                                 "outputTokens": cumulative[1],
                           "totalTokens": sum(cumulative)}}}}


def reasoning_snapshot_history():
    history = usage_snapshot_history()
    history["snapshot"]["state"]["reasoningEffort"] = {
        "reasoningEffort": "high", "source": os.environ.get("FAKE_REASONING_SOURCE", "policy")}
    return history


TODO_ITEMS = [
    {"text": "Read the schema", "status": "completed"},
    {"text": "Map todo lists", "status": "inProgress",
     "activeForm": "Mapping todo lists"},
    {"text": "Add tests", "status": "pending"},
    {"text": "Dropped task", "status": "cancelled"},
]


def question_params(user_input_id="ui-1"):
    questions = [{
        "id": "q0", "header": "Pick", "question": "Which?",
        "selection": {"mode": "single"},
        "options": [{"label": "Alpha"}, {"label": "Beta"}],
    }]
    if SCENARIO == "questions_multiple":
        questions[0]["selection"] = {"mode": "multiple", "minSelections": 1, "maxSelections": 2}
    if os.environ.get("FAKE_QUESTION_SHAPE", "") == "mixed":
        questions.extend([
            {"id": "q1", "header": "Pick many", "question": "Which ones?",
             "selection": {"mode": "multiple", "minSelections": 1,
                            "maxSelections": 2},
             "options": [{"label": "Red"}, {"label": "Blue"}]},
            {"id": "q2", "header": "Explain", "question": "Why?",
             "selection": {"mode": "single"}},
        ])
    return {"sessionId": MSP_SID, "userInputId": user_input_id,
            "turnId": "turn-question", "itemId": f"item-{user_input_id}",
            "toolCallId": f"call-{user_input_id}", "toolName": "request_user_input",
            "viewCursor": "cur-8", "questions": questions}


def on_turn_start(params):
    tid = turn_id()
    base = {"sessionId": ACTIVE_SESSION[0], "turnId": tid}
    if SCENARIO in ("happy", "malformed_msp"):
        notify("item/completed", {**base, "item": {
            "itemId": "it-1", "kind": "agentMessage",
            "status": "completed", "text": "hello from fake host"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "status_flags":
        notify("session/statusChanged", {
            "sessionId": MSP_SID, "status": "paused",
            "attention": ["futureAttention"], "viewCursor": None})
        notify("session/statusChanged", {
            "sessionId": MSP_SID, "status": "idle",
            "viewCursor": "cur-status-2"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "session_metadata":
        notify("session/nameChanged", {
            "sessionId": ACTIVE_SESSION[0], "name": "Renamed by host",
        })
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "failed":
        failed_params = {**base, "terminal": "failed"}
        err_kind = os.environ.get("FAKE_TURN_ERROR_KIND", "")
        err_msg = os.environ.get("FAKE_TURN_ERROR_MESSAGE", "")
        err_retry = os.environ.get("FAKE_TURN_ERROR_RETRYABLE", "")
        turn_reason = os.environ.get("FAKE_TURN_REASON", "")
        if err_kind or err_msg or err_retry:
            err_obj = {}
            if err_kind:
                err_obj["kind"] = err_kind
            else:
                err_obj["kind"] = "modelError"
            err_obj["message"] = err_msg if err_msg else "boom"
            if err_retry == "true":
                err_obj["retryable"] = True
            elif err_retry == "false":
                err_obj["retryable"] = False
            else:
                err_obj["retryable"] = False
            failed_params["error"] = err_obj
        if turn_reason:
            failed_params["reason"] = turn_reason
        notify("turn/completed", failed_params)
    elif SCENARIO == "deferred_launch_error":
        notify("turn/completed", {
            **base,
            "terminal": "failed",
            "reason": "queued turn launch failed: provider unavailable",
            "error": {
                "kind": "launchError",
                "message": "queued turn launch failed: provider unavailable",
                "retryable": True,
            },
        })
    elif SCENARIO == "tool":
        notify("item/completed", {**base, "item": {
            "itemId": "it-t1", "kind": "toolCall", "callId": "call-1",
            "status": "completed", "tool": "read",
            "args": {"path": "/tmp/x"}, "result": "file bytes"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "file_changes":
        changes = [
            ("it-add", "write_file", {"path": "added.bin"}),
            ("it-edit", "edit_file", {"path": "src/edited.rs"}),
            ("it-delete", "delete_file", {"path": "deleted.txt"}),
            ("it-rename", "rename_file",
             {"oldPath": "old.txt", "newPath": "new.txt"}),
        ]
        for item_id, tool, args in changes:
            event = {**base, "item": {
                "itemId": item_id, "kind": "toolCall", "callId": "call-" + item_id,
                "turnId": tid, "status": "completed", "tool": tool,
                "args": json.dumps(args)}}
            notify("item/completed", event)
            if item_id == "it-edit":
                notify("item/completed", event)  # gap/resume replay
        notify("item/completed", {**base, "item": {
            "itemId": "it-rejected", "kind": "toolCall", "callId": "call-no",
            "turnId": tid, "status": "rejected", "tool": "write_file",
            "args": json.dumps({"path": "not-written.txt"})}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "file_changes_subagent":
        notify("item/completed", {**base, "item": {
            "itemId": "it-child", "kind": "subagent", "turnId": tid,
            "status": "completed", "childSessionId": "child-session"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "file_changes_gap":
        notify("view/gap", {**base, "viewCursor": "gap-cursor"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "file_changes_ambiguous":
        notify("item/completed", {**base, "item": {
            "itemId": "it-shell", "kind": "toolCall", "callId": "call-shell",
            "turnId": tid, "status": "completed", "tool": "shell",
            "args": json.dumps({"command": "printf data > inferred.txt"})}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("approval", "pending_reconcile_dup"):
        notify("approval/requested", approval_params())
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("approval_hang", "approval_queue"):
        # The turn stays open until the client answers or cancels.
        notify("approval/requested", approval_params())
        if SCENARIO == "approval_queue":
            notify("approval/requested", approval_params(approvalId="approval-second"))
    elif SCENARIO == "approval_resolved_elsewhere":
        # Another actor (policy, reviewer, or a second client) decides the
        # approval while the editor still shows the permission prompt.
        notify("approval/requested", approval_params())
        notify("approval/resolved", {
            "sessionId": MSP_SID, "approvalId": "ap-1", "itemId": "it-ap",
            "turnId": tid, "decision": "approved", "resolvedBy": "policy",
            "policyResult": "allowed", "stageEvidence": [],
            "sourceRange": {"start": 0, "end": 1}, "viewCursor": "cur-ap"})
    elif SCENARIO == "child_approval_resolved":
        notify("item/started", {**base, "item": {
            "itemId": "it-sub-control", "kind": "subagent",
            "status": "inProgress", "revision": 1, "subagentId": "sub-control",
            "agentPath": "researcher", "depth": 1,
            "objective": "hold the approval test open",
            "childSessionId": "child-sess-control", "controlStatus": "running"}})
        notify("approval/requested", dict(
            APPROVAL_PARAMS, sessionId="child-sess-control"))
        notify("approval/resolved", {
            "sessionId": "child-sess-control", "approvalId": "ap-1",
            "itemId": "it-ap", "turnId": tid, "decision": "approved",
            "resolvedBy": "policy", "policyResult": "allowed",
            "stageEvidence": [], "sourceRange": {"start": 0, "end": 1},
            "viewCursor": "cur-child-ap"})
    elif SCENARIO == "question_settled_elsewhere":
        notify("userInput/requested", question_params("ui-1"))
        notify("userInput/settled", {
            "sessionId": MSP_SID, "userInputId": "ui-1", "outcome": "cancelled",
            "answers": [], "clarification": None, "decidedByCommandId": None,
            "reason": "answered in another client",
            "sourceRange": {"start": 0, "end": 1}, "viewCursor": "cur-settled"})
    elif SCENARIO == "approval_req":
        # Reissued multi-stage style: a server-initiated REQUEST with its
        # own id, no notification. Adapter must ack it AND bridge it.
        send({"jsonrpc": "2.0", "id": 9100, "method": "approval/request",
              "params": dict(APPROVAL_PARAMS)})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("questions", "questions_multiple", "questions_resume"):
        qid = "ui-2" if SCENARIO == "questions_resume" else "ui-1"
        if (USER_INPUT_DIALOGS[0]
                or os.environ.get("FAKE_IGNORE_USER_INPUT_DIALOGS") == "1"):
            notify("userInput/requested", question_params(qid))
            if SCENARIO == "questions_resume":
                # The request and view notification also describe the same ask.
                send({"jsonrpc": "2.0", "id": 9200, "method": "userInput/request",
                      "params": question_params(qid)})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "tool_huge_output":
        notify("item/completed", {**base, "item": {
            "itemId": "it-big", "kind": "toolCall", "callId": "call-big",
            "status": "completed", "tool": "read",
            "args": {"path": "/tmp/big"}, "result": "START" + "x" * 19980 + "FAILURE AT END!"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "tool_host_truncated":
        notify("item/completed", {**base, "item": {
            "itemId": "it-ht", "kind": "toolCall", "callId": "call-ht",
            "status": "completed", "tool": "read",
            "args": {"path": "/tmp/ht"}, "result": "short bounded text",
            "truncated": True}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("tool_stored_output", "tool_output_unavailable"):
        availability = "missing" if SCENARIO == "tool_output_unavailable" else "available"
        notify("item/completed", {**base, "item": {
            "itemId": "it-stored", "kind": "toolCall", "callId": "call-stored",
            "status": "completed", "tool": "apply_patch",
            "args": {"path": "/tmp/x"}, "visibleOutput": "bounded prefix",
            "truncated": True,
            "outputRef": {"availability": availability, "byteLen": 20,
                           "id": "out-1", "kind": "tool_output",
                           "mediaType": "text/plain", "uri": "muse://out-1"},
            "patchRef": {"availability": availability, "byteLen": 31,
                          "id": "patch-1", "kind": "tool_patch",
                          "mediaType": "application/json", "uri": "muse://patch-1"},
            "patchSummary": {"files": 2, "added": 4, "removed": 1}}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "host_exit_quiet":
        # Crash immediately after the turn/start ack: the turn is in flight
        # when the host dies, and the replacement host reports an idle fold.
        CRASH_AFTER_ACK[0] = True
    elif SCENARIO in ("host_exit_classified", "host_exit_relaunch_unavailable"):
        # Exit after the admission ack so the adapter can classify the
        # successful spawn separately from a launch/spawn failure.
        CRASH_AFTER_ACK[0] = True
    elif SCENARIO == "host_crash_loop":
        # First generation: crash after a turn ack. Every replacement crashes
        # right after its session/resume ack (see result_for), so the host
        # never stays up and the adapter must stop relaunching it.
        CRASH_AFTER_ACK[0] = True
    elif SCENARIO == "host_exit":
        # Complete the turn, then die like a crashed host once the ack is on
        # the wire: the adapter must restart, re-attach, and keep serving.
        notify("item/completed", {**base, "item": {
            "itemId": "it-hx", "kind": "agentMessage", "status": "completed",
            "text": "before the crash"}})
        notify("turn/completed", {**base, "terminal": "completed"})
        CRASH_AFTER_ACK[0] = True
    elif SCENARIO == "view_health":
        notify("session/viewHealthChanged", {
            "sessionId": MSP_SID, "health": "unavailable",
            "noneReason": "projectionUnavailable"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "todo":
        notify("session/todoListChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-t1",
            "revision": 1, "sourceTool": "todo_write",
            "items": TODO_ITEMS})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "todo_cleared":
        notify("session/todoListChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-t1",
            "revision": 1, "sourceTool": "todo_write", "items": TODO_ITEMS})
        notify("session/todoListChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-t2",
            "revision": 2, "sourceTool": "todo_write", "items": []})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "reasoning_stream":
        rid = "it-r1"
        notify("item/started", {**base, "item": {
            "itemId": rid, "kind": "reasoning", "status": "inProgress",
            "revision": 1}})
        notify("item/delta", {"sessionId": MSP_SID, "itemId": rid,
                              "field": "summary.0", "delta": "Considering ",
                              "viewCursor": "cur-r1"})
        notify("item/delta", {"sessionId": MSP_SID, "itemId": rid,
                              "field": "summary.0", "delta": "the schema",
                              "viewCursor": "cur-r2"})
        notify("item/delta", {"sessionId": MSP_SID, "itemId": rid,
                              "field": "summary.1", "delta": "Then testing",
                              "viewCursor": "cur-r3"})
        notify("item/completed", {**base, "item": {
            "itemId": rid, "kind": "reasoning", "status": "completed",
            "revision": 2, "truncated": False,
            "summary": ["Considering the schema", "Then testing"]}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m1", "kind": "agentMessage", "status": "completed",
            "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "subagent_native":
        sid_item = "it-subn"
        notify("item/started", {**base, "item": {
            "itemId": sid_item, "kind": "subagent", "status": "inProgress",
            "revision": 1, "subagentId": "sub-n1", "agentPath": "researcher",
            "depth": 1, "objective": "survey failing tests",
            "childSessionId": "child-sess-native", "controlStatus": "running"}})
        notify("item/completed", {**base, "item": {
            "itemId": sid_item, "kind": "subagent", "status": "completed",
            "revision": 2, "subagentId": "sub-n1", "agentPath": "researcher",
            "depth": 1, "objective": "survey failing tests",
            "childSessionId": "child-sess-native", "controlStatus": "closed",
            "result": {"summary": "native child finished",
                       "evidenceRefs": [], "artifactRefs": []}}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "subagent_control":
        control = os.environ.get("FAKE_SUBAGENT_CONTROL_STATUS", "running")
        item_status = os.environ.get("FAKE_SUBAGENT_ITEM_STATUS", "inProgress")
        notify("item/started", {**base, "item": {
            "itemId": "it-sub-control", "kind": "subagent",
            "status": item_status, "revision": 1, "subagentId": "sub-control",
            "agentPath": "researcher", "depth": 1,
            "objective": "hold the control test open",
            "childSessionId": "child-sess-control",
            "controlStatus": control}})
    elif SCENARIO == "subagent_child_approval":
        notify("item/started", {**base, "item": {
            "itemId": "it-sub-control", "kind": "subagent",
            "status": "inProgress", "revision": 1, "subagentId": "sub-control",
            "agentPath": "researcher", "depth": 1,
            "objective": "hold the approval test open",
            "childSessionId": "child-sess-control", "controlStatus": "running"}})
        notify("approval/requested", dict(
            APPROVAL_PARAMS, sessionId="child-sess-control", availableChoices=[]))
    elif SCENARIO == "subagent":
        sid_item = "it-sub1"
        notify("item/started", {**base, "item": {
            "itemId": sid_item, "kind": "subagent", "status": "inProgress",
            "revision": 1, "subagentId": "sub-1", "agentPath": "researcher",
            "depth": 1, "objective": "survey failing tests",
            "childSessionId": "child-sess-1", "controlStatus": "running"}})
        notify("item/completed", {**base, "item": {
            "itemId": sid_item, "kind": "subagent", "status": "completed",
            "revision": 2, "subagentId": "sub-1", "agentPath": "researcher",
            "depth": 1, "objective": "survey failing tests",
            "childSessionId": "child-sess-1", "controlStatus": "closed",
            "result": {"summary": "child finished", "evidenceRefs": [],
                       "artifactRefs": []}}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "workflow":
        wf = "it-wf1"
        notify("item/updated", {**base, "item": {
            "itemId": wf, "kind": "workflow", "status": "inProgress",
            "revision": 2, "workflowRunId": "wfr-1", "entryId": "triage-batch",
            "scriptId": "triage@sha256:aa10", "triggerSource": "modelProposal",
            "children": [{"childId": "c1", "attempt": 1, "status": "started",
                          "phase": "triage", "label": "triage issue #1"}]}})
        notify("item/completed", {**base, "item": {
            "itemId": wf, "kind": "workflow", "status": "completed",
            "revision": 3, "workflowRunId": "wfr-1", "entryId": "triage-batch",
            "scriptId": "triage@sha256:aa10", "triggerSource": "modelProposal",
            "children": [{"childId": "c1", "attempt": 1, "status": "completed",
                          "label": "triage issue #1"}]}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "workflow_control":
        wf = "it-wf-control"
        WORKFLOW_TURN[0] = tid
        notify("item/updated", {**base, "item": {
            "itemId": wf, "kind": "workflow", "status": "inProgress",
            "revision": 2, "workflowRunId": "wfr-control",
            "entryId": "triage-batch", "scriptId": "triage@sha256:aa10",
            "triggerSource": "modelProposal",
            "children": [{"childId": "c1", "attempt": 1, "status": "started",
                          "phase": "triage", "label": "triage issue #1"}]}})
    elif SCENARIO == "async_task":
        # A backgrounded toolCall plus a user-shell item: both are async work.
        notify("item/updated", {**base, "item": {
            "itemId": "it-bg1", "kind": "toolCall", "callId": "call-bg1",
            "status": "inProgress", "revision": 2, "tool": "workspace-shell",
            "args": {"command": "npm watch"}, "background": True,
            "backgroundInitiator": "user"}})
        notify("item/started", {"sessionId": MSP_SID, "item": {
            "itemId": "it-sh2", "kind": "userShell", "status": "inProgress",
            "revision": 1, "turnId": None, "commandText": "cargo watch"},
            "viewCursor": "cur-sh2"})
        notify("item/completed", {"sessionId": MSP_SID, "item": {
            "itemId": "it-sh2", "kind": "userShell", "status": "completed",
            "revision": 2, "turnId": None, "commandText": "cargo watch",
            "exitSignal": 9, "visibleOutput": "watching"},
            "viewCursor": "cur-sh3"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "async_task_stop":
        # Leave a background task live so the ACP task-stop path can target it.
        notify("item/updated", {**base, "item": {
            "itemId": "it-bg-stop", "kind": "toolCall", "callId": "call-bg-stop",
            "status": "inProgress", "revision": 1, "tool": "workspace-shell",
            "args": {"command": "npm watch"}, "background": True,
            "backgroundInitiator": "user"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "async_task_stop_all":
        # Keep two background tasks live until ACP session/cancel maps to the
        # MSP blanket command.
        for item_id, call_id, command in [
            ("it-bg-all-1", "call-bg-all-1", "npm watch"),
            ("it-bg-all-2", "call-bg-all-2", "cargo watch"),
        ]:
            notify("item/updated", {**base, "item": {
                "itemId": item_id, "kind": "toolCall", "callId": call_id,
                "status": "inProgress", "revision": 1, "tool": "workspace-shell",
                "args": {"command": command}, "background": True,
                "backgroundInitiator": "user"}})
    elif SCENARIO == "usershell_item":
        notify("item/completed", {"sessionId": MSP_SID, "item": {
            "itemId": "it-sh1", "kind": "userShell", "status": "completed",
            "revision": 1, "turnId": None, "commandText": "git status",
            "exitCode": 0, "visibleOutput": "## main", "durationMs": 120},
            "viewCursor": "cur-sh1"})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m2", "kind": "agentMessage", "status": "completed",
            "text": "shell done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "unknown_kind":
        notify("item/completed", {**base, "item": {
            "itemId": "it-u1", "kind": "hologramPreview", "status": "completed",
            "revision": 1, "fallbackText": "Previewed a hologram"}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-u2", "kind": "mysteryKind", "status": "completed",
            "revision": 1}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m3", "kind": "agentMessage", "status": "completed",
            "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "goal_branch":
        notify("session/goalChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-g1",
            "goal": {"objective": "Green the suite", "status": "active",
                     "percentComplete": 42,
                     "currentWork": "Fixing fold tests",
                     "nextWork": "Re-run CI"}})
        notify("session/branchChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-b1",
            "branch": "feat/msp", "vcs": "git",
            "workspaceRoot": "/home/me/src/proj"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "goal_clear":
        notify("session/goalChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-g1",
            "goal": {"objective": "Old", "status": "active",
                     "percentComplete": 10}})
        notify("session/goalChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-g2", "goal": None})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "reasoning_quiet":
        # No deltas: the completed summary must still be emitted once.
        notify("item/completed", {**base, "item": {
            "itemId": "it-r9", "kind": "reasoning", "status": "completed",
            "revision": 1, "summary": ["Committed thought"]}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m9", "kind": "agentMessage", "status": "completed",
            "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "queued":
        if TURNS[0] == 2:
            notify("turn/completed", {"sessionId": MSP_SID,
                                     "turnId": "turn-1",
                                     "terminal": "completed"})
            notify("turn/completed", {"sessionId": MSP_SID,
                                     "turnId": "turn-2",
                                     "terminal": "completed"})
    elif SCENARIO == "cancel_request":
        # Leave the running turn and all queued turns open. The test drives
        # the targeted reclaim, then uses session/cancel to clean up the
        # remaining turns and prove they were left alone.
        pass
    elif SCENARIO == "unqueued":
        notify("turn/unqueued", dict(base))
    elif SCENARIO == "retracted":
        # The submission is durably retracted: no turn/completed follows.
        notify("turn/retracted", {**base, "commandId": params.get("commandId", "")})
    elif SCENARIO == "retract_then_completed":
        # A terminal still arrives after the retract: settle exactly once.
        notify("turn/retracted", {**base, "commandId": params.get("commandId", "")})
        notify("turn/completed", {**base, "terminal": "cancelled"})
    elif SCENARIO == "retry_then_completed":
        # Retry is non-terminal: the later completion settles, exactly once.
        notify("turn/retryScheduled", {**base,
                                      "attempt": 1, "nextAttempt": 2,
                                      "maxAttempts": 3, "retryDelayMs": 2000,
                                      "reason": "provider stream disconnected"})
        notify("item/completed", {**base, "item": {
            "itemId": "it-rt", "kind": "agentMessage",
            "status": "completed", "text": "recovered"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage":
        # A tokenUsage before any contextUsage has no used/size pair and
        # must be held back, not emitted with nulls.
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 100, "totalTokens": 120, "modelId": "fake-model",
            "usage": {"inputTokens": 100, "outputTokens": 20,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": "cur-0", "sourceRange": {"start": 0, "end": 1},
            "cumulative": cumulative_totals(100, 20)})
        notify("session/contextUsage", {
            "sessionId": MSP_SID, "usedTokens": 1234, "windowTokens": 200000,
            "pressure": "normal", "viewCursor": "cur-1",
            "sourceRange": {"start": 0, "end": 1}})
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 1000, "totalTokens": 1500, "modelId": "fake-model",
            "usage": {"inputTokens": 1000, "outputTokens": 500,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": "cur-2", "sourceRange": {"start": 0, "end": 2},
            "cumulative": cumulative_totals(5000, 2500)})
        # Pre-schema record: no modelId, so an unpriced leg. Totals still
        # advance; the running cost must not.
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 1000, "totalTokens": 1500,
            "usage": {"inputTokens": 1000, "outputTokens": 500,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": "cur-3", "sourceRange": {"start": 0, "end": 3},
            "cumulative": cumulative_totals(6000, 3000)})
        notify("item/completed", {**base, "item": {
            "itemId": "it-1", "kind": "agentMessage",
            "status": "completed", "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_cached":
        # Cached tokens are a subset of prompt tokens: the estimate must
        # charge them at the catalog cached rate, not the full input rate.
        notify("session/contextUsage", context_usage(1500, "cur-1"))
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 1000, "totalTokens": 1500, "modelId": "fake-model",
            "usage": {"inputTokens": 1000, "outputTokens": 500,
                      "cachedTokens": 800, "reasoningTokens": 0},
            "viewCursor": "cur-2", "sourceRange": {"start": 0, "end": 2},
            "cumulative": {"promptTokens": 1000, "outputTokens": 500,
                           "totalTokens": 1500}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("subscription_usage", "subscription_first_observation"):
        # The notification is host-global and carries no sessionId. The
        # adapter must attach it to every ACP session without treating it as
        # token usage or a local cost estimate.
        notify("session/contextUsage", context_usage(1500, "cur-1"))
        notify("usage/changed", subscription_usage(23, 61))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_turn":
        # Two model calls in one turn, from two models, plus a replay of the
        # second leg's view cursor. The prompt result must carry the sum of
        # the two distinct legs, the replay counted once.
        notify("session/contextUsage", context_usage(1500, "cur-1"))
        leg_a = {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 1000, "totalTokens": 1500, "modelId": "fake-model",
            "durationMs": 1200,
            "usage": {"inputTokens": 1000, "outputTokens": 500,
                      "cachedTokens": 200, "reasoningTokens": 50,
                      "cacheReadTokens": 200, "cacheWriteTokens": 30},
            "viewCursor": "cur-2", "sourceRange": {"start": 0, "end": 2},
            "cumulative": {"promptTokens": 1000, "outputTokens": 500,
                           "totalTokens": 1500}}
        leg_b = {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 400, "totalTokens": 700, "modelId": "other-model",
            "durationMs": 800,
            "usage": {"inputTokens": 400, "outputTokens": 300,
                      "cachedTokens": 0, "reasoningTokens": 10},
            "viewCursor": "cur-3", "sourceRange": {"start": 0, "end": 3},
            "cumulative": {"promptTokens": 1400, "outputTokens": 800,
                           "totalTokens": 2200}}
        notify("session/tokenUsage", leg_a)
        notify("session/tokenUsage", leg_b)
        notify("session/tokenUsage", dict(leg_b))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_turn_cancelled":
        # One leg, then a cancelled terminal: the settled prompt result still
        # reports what the turn spent.
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 1000, "totalTokens": 1500, "modelId": "fake-model",
            "durationMs": 700,
            "usage": {"inputTokens": 1000, "outputTokens": 500,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": "cur-2", "sourceRange": {"start": 0, "end": 2},
            "cumulative": {"promptTokens": 1000, "outputTokens": 500,
                           "totalTokens": 1500}})
        notify("turn/completed", {**base, "terminal": "cancelled"})
    elif SCENARIO == "gap_after_next":
        # A real host flushes the gap bracket at its next ACCEPTED delivery,
        # so `next` (cur-3) is folded before the view/gap that names the
        # hole. cur-2 was dropped from the live stream.
        notify("item/completed", gap_message(tid, "A", "cur-1"))
        notify("item/completed", gap_message(tid, "C", "cur-3"))
        notify("view/gap", {"sessionId": MSP_SID,
                            "after": "cur-1", "next": "cur-3"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "gap_ephemeral_next":
        # `next` names an ephemeral item/delta, which view/page never serves.
        # The turn's durable terminal follows the gap on the live stream.
        notify("item/completed", gap_message(tid, "A", "cur-1"))
        notify("item/delta", {"sessionId": MSP_SID, "itemId": "gap-streaming",
                              "delta": "partial", "viewCursor": "cur-3"})
        notify("view/gap", {"sessionId": MSP_SID,
                            "after": "cur-1", "next": "cur-3"})
        notify("turn/completed", {**base, "terminal": "completed",
                                  "viewCursor": "cur-5"})
    elif SCENARIO == "gap_cursor_cycle":
        notify("item/completed", gap_message(tid, "A", "cur-1"))
        notify("item/completed", gap_message(tid, "C", "cur-9"))
        notify("view/gap", {"sessionId": MSP_SID,
                            "after": "cur-1", "next": "cur-9"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_gap":
        # cur-3 is delivered twice: once by the view/gap refill page and
        # once on the live stream. Two distinct completions, one price each.
        notify("session/contextUsage", context_usage(120, "cur-1"))
        notify("view/gap", {"sessionId": MSP_SID,
                            "after": "cur-1", "next": "cur-3"})
        notify("session/tokenUsage", token_usage("cur-3", 1000, 500, 1100, 520))
        notify("session/contextUsage", context_usage(1620, "cur-4"))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("usage_rates_dropped", "usage_rates_empty",
                      "usage_rates_invalid", "usage_rates_failure"):
        if TURNS[0] == 1:
            notify("session/contextUsage", context_usage(120, "cur-1"))
        notify("session/tokenUsage", token_usage(
            f"cur-{TURNS[0] + 1}", 100, 20, 100 * TURNS[0], 20 * TURNS[0]))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_inline_nosnapshot":
        # The occupancy only ever arrives live; the totals must already be
        # the ones read back from the durable page, never null.
        notify("session/contextUsage", context_usage(1500, "cur-9"))
        notify("session/tokenUsage", token_usage("cur-10", 100, 20, 400, 80))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_snapshot_null":
        # Turn 1 establishes real occupancy; turn 2 runs after a reattach
        # whose snapshot has no contextUsage, and reports no new occupancy.
        if TURNS[0] == 1:
            notify("session/contextUsage", context_usage(120, "cur-1"))
        notify("session/tokenUsage", token_usage(
            f"cur-{TURNS[0] + 1}", 100, 20, 100 * TURNS[0], 20 * TURNS[0]))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_resume":
        # Occupancy is unchanged after the resume, so the host emits no
        # contextUsage: only the restored window makes this leg sendable.
        notify("session/tokenUsage", token_usage("cur-10", 100, 20, 200, 40))
        notify("turn/completed", {**base, "terminal": "completed"})
    disposition = (
        "queued"
        if SCENARIO == "deferred_launch_error"
        or (SCENARIO in ("queued", "cancel_request") and TURNS[0] > 1)
        else "started"
    )
    return {
        "commandId": params.get("commandId", ""),
        "status": "accepted",
        "turnId": tid,
        "disposition": disposition,
        "startedNewTurn": disposition == "started",
    }


def result_for(method, msg):
    if REVIEW_HOST:
        if method == "session/start":
            return {"session": session_obj(session_id=REVIEW_SID),
                    "viewCursor": "cur-review"}
        if method == "turn/start":
            log_input(msg.get("params", {}))
            return {"status": "accepted"}
    if method == "initialize":
        requested = (msg.get("params", {}).get("capabilities", {})
                     .get("requestedCapabilities", []))
        granted = (["sessionListStream"]
                   if SCENARIO == "session_list_stream"
                   and "sessionListStream" in requested else [])
        if SCENARIO.startswith("user_shell") and "userShell" in requested:
            granted.append("userShell")
        SESSION_MCP[0] = ("sessionMcp" in requested
                          and os.environ.get("FAKE_NO_SESSION_MCP") != "1")
        if SESSION_MCP[0]:
            granted.append("sessionMcp")
        FEEDBACK[0] = ("feedback" in requested
                       and os.environ.get("FAKE_NO_FEEDBACK") != "1")
        if FEEDBACK[0]:
            granted.append("feedback")
        USER_INPUT_DIALOGS[0] = msg.get("params", {}).get("capabilities", {}).get(
            "userInputDialogs", True) is not False
        EXPERIMENTAL_API[0] = msg.get("params", {}).get("capabilities", {}).get(
            "experimentalApi") is True
        result = {
            "schema": SCHEMA,
            "grantedCapabilities": granted,
            "serverInfo": {
                "name": "muse-session-server-fixture",
                "version": os.environ.get("FAKE_SERVER_VERSION", "0.0.0-fixture"),
            },
        }
        if os.environ.get("FAKE_SESSION_DURABILITY", ""):
            result["sessionDurability"] = os.environ["FAKE_SESSION_DURABILITY"]
        return result
    if method == "approval/listPending":
        if SCENARIO == "pending_reconcile":
            return {"approvals": [dict(APPROVAL_PARAMS, approvalId="ap-reconcile")],
                    "userInputs": [question_params("ui-reconcile")]}
        if SCENARIO == "pending_reconcile_dup":
            # The same approval the adapter is already displaying.
            return {"approvals": [dict(APPROVAL_PARAMS)], "userInputs": []}
        if SCENARIO == "status_flags":
            return {"approvals": [dict(APPROVAL_PARAMS, approvalId="ap-status")],
                    "userInputs": [question_params("ui-status")]}
        return {"approvals": [], "userInputs": []}
    if method == "usage/read":
        USAGE_READS[0] += 1
        if SCENARIO == "subscription_usage":
            return {"usage": subscription_usage(11, 37)}
        return {}
    if method == "session/start":
        workspace_root = msg.get("params", {}).get("workspaceRoot", "/tmp/fake-ws")
        ACTIVE_WORKSPACE[0] = workspace_root
        result = {"session": session_obj(), "viewCursor": "cur-0"}
        if SCENARIO == "rename_live":
            result["session"]["name"] = "Before rename"
        elif SCENARIO == "rename_resume":
            result["session"]["name"] = "Before resume"
        elif SCENARIO == "rename_clear":
            result["session"]["name"] = "Before clear"
        return result
    if method == "skill/list":
        log_input(msg.get("params", {}))
        SKILL_READS[0] += 1
        if (SCENARIO == "skills_changed" and SKILL_READS[0] > 1
                and os.environ.get("FAKE_SKILL_REFRESH_FAILS") == "1"):
            return {}
        if SCENARIO == "skills_changed" and SKILL_READS[0] > 1:
            return {"skills": [{
                "selector": "review",
                "description": "Review the current changes",
                "displayName": "Review",
                "argumentHint": "what to review",
                "source": "project",
            }]}
        skills = [{
            "selector": "plan",
            "description": "Create a grounded plan",
            "displayName": "Plan",
            "argumentHint": "what to plan",
            "source": "bundled",
        }]
        if SCENARIO == "skill_not_found":
            # The catalog read races the skill's removal: the adapter still
            # lists `stale`, but the host no longer resolves it.
            skills.append({"selector": "stale", "description": "Removed",
                           "displayName": "Stale", "source": "project"})
        return {"skills": skills}
    if method == "item/readOutput":
        params = msg.get("params", {})
        offset = params.get("offsetBytes", 0)
        return {"byteLen": 18, "content": "full stored output",
                "encoding": "utf8", "eof": True,
                "mediaType": "text/plain", "offsetBytes": offset}
    if method == "session/resume":
        if SCENARIO == "host_crash_loop":
            CRASH_AFTER_ACK[0] = True
        params = msg.get("params", {})
        log_input(params)
        # Only the explicitly requested snapshot rung can carry occupancy;
        # the default `auto` rung resolves to inline on the real host.
        snapshot_rung = params.get("history") == "snapshot"
        if SCENARIO == "usage_resume":
            history = usage_snapshot_history()
        elif SCENARIO == "reasoning_resume":
            history = reasoning_snapshot_history()
        elif SCENARIO == "todo_resume":
            history = usage_snapshot_history()
            history["snapshot"]["state"]["todoList"] = {
                "items": TODO_ITEMS, "revision": 3,
                "sourceTool": "todo_write"}
        elif SCENARIO == "goal_branch_resume":
            history = usage_snapshot_history()
            history["snapshot"]["state"]["goal"] = {
                "objective": "Snapshot goal", "status": "paused",
                "percentComplete": 5}
            history["snapshot"]["state"]["branch"] = {
                "branch": "main", "vcs": "git",
                "workspaceRoot": "/home/me/src/proj"}
        elif SCENARIO == "usage_snapshot_null":
            history = usage_snapshot_history(context=False)
        elif SCENARIO == "rename_snapshot":
            history = usage_snapshot_history()
            history["snapshot"]["state"]["name"] = "Snapshot name"
        elif SCENARIO == "user_shell_recovered":
            history = {"mode": "inline", "snapshot": None, "items": [{
                "itemId": "shell-explicit", "kind": "userShell", "turnId": None,
                "commandText": "sleep 1", "status": "completed", "revision": 2,
                "exitCode": 7, "visibleOutput": "recovered shell terminal"}]}
        elif SCENARIO == "async_resume":
            history = {"mode": "inline", "snapshot": None, "items": [
                {"itemId": "bg-resumed", "kind": "toolCall",
                 "callId": "call-bg-resumed", "status": "inProgress",
                 "tool": "workspace-shell", "args": {"command": "npm watch"},
                 "background": True, "backgroundInitiator": "timeout"},
                {"itemId": "shell-resumed", "kind": "userShell",
                 "status": "inProgress", "turnId": None,
                 "commandText": "cargo watch"},
                {"itemId": "shell-done", "kind": "userShell",
                 "status": "completed", "turnId": None,
                 "commandText": "old command", "exitCode": 0},
            ]}
        elif SCENARIO in ("usage_inline", "questions_resume") and snapshot_rung:
            history = usage_snapshot_history(cumulative=(300, 60))
        else:
            history = {"mode": "inline", "items": history_items(),
                       "snapshot": None}
        pending = []
        if SCENARIO == "questions_resume":
            pending = [{"kind": "userInput", "userInputId": "ui-1",
                        "viewCursor": "cur-8"}]
            if history.get("snapshot"):
                history["snapshot"]["state"]["pendingUserInputs"] = [
                    {"userInputId": "ui-1", "itemId": "item-ui-1",
                     "viewCursor": "cur-8"}]
        workspace_root = "/tmp" if SCENARIO == "resume_active" else None
        session = session_obj(params.get("sessionId", MSP_SID), workspace_root)
        if SCENARIO == "cancel_request" and os.environ.get("FAKE_RESUME_QUEUED"):
            session["activeTurnId"] = "turn-2"
        if SCENARIO == "rename_resume":
            session["name"] = "Renamed outside adapter"
        elif SCENARIO == "rename_clear":
            session["name"] = None
        return {"session": session,
                "viewCursor": "cur-9",
                "pendingRequests": pending,
                "history": history}
    if method == "view/subscribe":
        params = msg.get("params", {})
        log_input(params)
        return {"viewCursor": "cur-9"}
    if method == "view/page" and SCENARIO.startswith("fork_history"):
        params = msg.get("params", {})
        if SCENARIO == "fork_history_failure":
            return {"events": [], "nextCursor": "stuck"}
        def event(cursor, item):
            return {"method": "item/completed", "params": {
                "sessionId": ACTIVE_SESSION[0], "viewCursor": cursor, "item": item}}
        if not params.get("cursor"):
            return {"events": [event("fork-start", dict(FORK_ITEMS[0], text="partial"))],
                    "nextCursor": "fork-start"}
        events = [event("fork-start", dict(FORK_ITEMS[0], text="partial"))]
        events.extend(event("fork-head" if i == len(FORK_ITEMS) - 1 else "fork-middle", item)
                      for i, item in enumerate(FORK_ITEMS))
        events.append(event("after-fork-head", {"itemId": "too-late", "kind": "agentMessage",
                                               "text": "must not replay", "status": "completed"}))
        return {"events": events, "nextCursor": None}
    if method == "view/page":
        if SCENARIO == "file_changes_gap":
            return {"items": [{"itemId": "gap-write", "kind": "toolCall",
                "callId": "gap-call", "turnId": "turn-1", "status": "completed",
                "tool": "write_file", "args": {"path": "gap-written.txt"}}]}
        page = msg.get("params", {})
        log_input(page)
        if SCENARIO == "gap_after_next":
            # Two pages walk the hole; the second ends at `next`, which the
            # live stream already delivered, and one event beyond it.
            if page.get("cursor") == "cur-1":
                return {"events": [{"method": "item/completed",
                                    "params": gap_message("turn-1", "B", "cur-2")}],
                        "nextCursor": "cur-2"}
            if page.get("cursor") == "cur-2":
                return {"events": [
                    {"method": "item/completed",
                     "params": gap_message("turn-1", "C", "cur-3")},
                    {"method": "item/completed",
                     "params": gap_message("turn-1", "D", "cur-4")}],
                    "nextCursor": "cur-4"}
            return {"events": [], "nextCursor": None}
        if SCENARIO == "gap_ephemeral_next":
            if page.get("cursor") == "cur-1":
                return {"events": [
                    {"method": "item/completed",
                     "params": gap_message("turn-1", "B", "cur-2")},
                    {"method": "turn/completed",
                     "params": {"sessionId": MSP_SID, "turnId": "turn-1",
                                "terminal": "completed", "viewCursor": "cur-5"}}],
                    "nextCursor": None}
            return {"events": [], "nextCursor": None}
        if SCENARIO == "gap_cursor_cycle":
            # A misbehaving host: cur-1 -> cur-2 -> cur-1 -> ...
            if page.get("cursor") == "cur-1":
                return {"events": [{"method": "item/completed",
                                    "params": gap_message("turn-1", "B", "cur-2")}],
                        "nextCursor": "cur-2"}
            return {"events": [{"method": "item/completed",
                                "params": gap_message("turn-1", "X", "cur-3")}],
                    "nextCursor": "cur-1"}
        if SCENARIO == "usage_gap" and page.get("direction") != "backward":
            # Refill overlaps the live stream: cur-3 is in this page too.
            return {"events": [
                {"method": "session/tokenUsage",
                 "params": token_usage("cur-2", 100, 20, 100, 20)},
                {"method": "session/tokenUsage",
                 "params": token_usage("cur-3", 1000, 500, 1100, 520)}],
                "nextCursor": "cur-3"}
        if (SCENARIO in ("usage_inline", "usage_inline_nosnapshot")
                and page.get("direction") == "backward"):
            # Ascending by viewCursor, as MSP guarantees in both directions.
            # No session/contextUsage: it is not durable-sourced, so it never
            # appears in a page -- verified against a real Muse 1.0.3 host.
            return {"events": [
                {"method": "session/tokenUsage",
                 "params": token_usage("cur-8", 300, 60, 300, 60)}],
                "nextCursor": None}
        # Every other session pages back to nothing usable.
        return {"events": [], "nextCursor": None}
    if method == "session/list":
        params = msg.get("params", {})
        filter_id = ((params.get("filter") or {}).get("sessionId") or {}).get("anyOf")
        if SCENARIO == "session_list_workspace_filter":
            if params.get("workspaceRoot") == "/tmp/unrelated-ws":
                return {"sessions": [], "nextCursor": None}
        if SCENARIO == "session_list_pagination":
            if params.get("cursor") == "page-2":
                return {"sessions": [session_obj("stored-201", "/tmp/page-2")],
                        "nextCursor": None}
            return {"sessions": [
                        session_obj(f"stored-{n}", f"/tmp/session-{n}")
                        for n in range(1, 201)],
                    "nextCursor": "page-2"}
        if filter_id is not None:
            rows = [session_obj(), session_obj("msp-sess-old", "/tmp/old-ws"),
                    session_obj("msp-sess-untitled", "/tmp/untitled-ws"),
                    session_obj("msp-sess-bare", "/tmp/bare-ws")]
            rows = [row for row in rows
                    if row["sessionId"] in filter_id
                    and row["sessionId"] not in DELETED_SESSIONS]
            return {"sessions": rows,
                    "appliedFilter": params.get("filter"),
                    "nextCursor": None}
        live = session_obj()
        old = session_obj("msp-sess-old", "/tmp/old-ws")
        untitled = session_obj("msp-sess-untitled", "/tmp/untitled-ws")
        bare = session_obj("msp-sess-bare", "/tmp/bare-ws")
        old["updatedAt"] = "2026-08-01T00:00:00Z"
        live["updatedAt"] = "2026-09-04T00:00:00Z"
        rows = [live, old, untitled, bare]
        if os.environ.get("FAKE_LIST_NULL_ROOT") == "1":
            rows.append({"sessionId": "msp-sess-noroot", "workspaceRoot": None})
        rows = [row for row in rows if row["sessionId"] not in DELETED_SESSIONS]
        return {"sessions": rows, "nextCursor": None}
    if method == "session/delete":
        params = msg.get("params", {})
        log_input(params)
        return {"commandId": params.get("commandId", ""), "status": "accepted"}
    if method == "feedback/submit":
        params = msg.get("params", {})
        log_input(params)
        outcome = os.environ.get("FAKE_FEEDBACK_OUTCOME", "uploaded")
        result = {
            "bundlePath": "/tmp/fixture-feedback.zip",
            "outcome": outcome,
            "sessionRecordAttached": bool(params.get("attachSessionRecord")),
            "sessionRecordTruncated": False,
        }
        if outcome == "uploaded":
            result["uploadId"] = "fixture-upload-1"
        elif outcome == "rateLimited":
            result["retryAfterMs"] = 5000
        elif outcome in ("dark", "failed", "noCredential", "authRejected"):
            result["cause"] = "fixture cause"
        if os.environ.get("FAKE_FEEDBACK_NOTES") == "1":
            result["sessionNote"] = "Session note from the fixture"
            result["localTracingNote"] = "Tracing note from the fixture"
        return result
    if method == "session/setApprovalMode":
        # The real host applies the selected mode and echoes it back.
        mode = FOLDED_MODE or msg.get("params", {}).get("mode", MODE)
        return {"commandId": "x", "status": "ok", "applyOutcome": "applied",
                "effectiveMode": {"lastCommandId": "x", "mode": mode,
                                  "source": "explicit"}}
    if method == "session/setReasoningEffort":
        return {"commandId": msg.get("params", {}).get("commandId", ""),
                "status": "accepted"}
    if method == "model/list":
        CATALOG_READS[0] += 1
        if SCENARIO == "usage_rates_failure" and CATALOG_READS[0] > 1:
            return {}  # malformed: no models array, so the refresh failed
        if SCENARIO == "catalog_refresh_failure":
            if CATALOG_READS[0] == 2:
                return {}
            if CATALOG_READS[0] == 4:
                return {"models": [], "source": "unresolvedCatalog"}
        models = [{"modelId": "fake-model", "displayLabel": "Fake",
                   "isDefault": True,
                   "variants": ["low", "medium", "high"],
                   "reasoningEffortVariants": [
                       {"tier": "low", "description": "Quick answers"},
                       {"tier": "medium", "description": "Balanced effort"},
                       {"tier": "high", "description": "Deep reasoning"},
                   ],
                   "defaultReasoningEffort": "medium",
                   "cost": {"input": "3.00", "output": "15.00",
                            "cached": "0.30", "currency": "USD"}}]
        if os.environ.get("FAKE_VARIANTS_UNKNOWN") == "1":
            models[0]["variants"] = "unknown"
            models[0].pop("reasoningEffortVariants", None)
        if os.environ.get("FAKE_SECOND_MODEL") == "1":
            models.append({
                "modelId": "second-model", "displayLabel": "Second",
                "variants": ["minimal", "high", "xhigh"],
                "reasoningEffortVariants": [
                    {"tier": "minimal", "description": "Fastest"},
                    {"tier": "xhigh", "description": "Most thorough"},
                ],
                "defaultReasoningEffort": "xhigh",
            })
        if CATALOG_READS[0] > 1:
            if SCENARIO == "usage_rates_dropped":
                models[0]["cost"] = None
            elif SCENARIO == "usage_rates_empty":
                models = []
            elif SCENARIO == "usage_rates_invalid":
                models[0]["cost"]["input"] = "NaN"
        if SCENARIO == "catalog_grows" and CATALOG_READS[0] > 1:
            models.append({"modelId": "second-model", "displayLabel": "Second"})
        return {"models": models, "source": "fakeCatalog"}
    if method == "turn/start":
        params = msg.get("params", {})
        log_input(params)
        return on_turn_start(params)
    if method == "turn/unqueue":
        params = msg.get("params", {})
        log_input(params)
        if SCENARIO == "cancel_request":
            notify("turn/unqueued", {
                "sessionId": MSP_SID,
                "turnId": params.get("turnId", ""),
                "commandId": params.get("turnId", ""),
            })
        return {
            "commandId": params.get("commandId", ""),
            "status": "accepted",
            "turnId": params.get("turnId", ""),
        }
    if method == "session/read":
        read_params = msg.get("params", {})
        log_input(read_params)
        if read_params.get("sessionId") == "child-sess-native":
            return {"session": session_obj("child-sess-native"),
                    "history": {"mode": "inline", "items": [
                        {"itemId": "c-msg-1", "kind": "agentMessage",
                         "status": "completed",
                         "text": "child did the research"},
                        {"itemId": "c-tool-1", "kind": "toolCall",
                         "callId": "c-call-1", "status": "completed",
                         "tool": "read", "args": {"path": "/tmp/c"},
                         "result": "child bytes"},
                    ], "snapshot": None},
                    "viewCursor": "cur-child", "pendingRequests": []}
        # Items carry turnId so fork points can resolve to a completed turn.
        items = history_items() + [
            {"itemId": "msg-fork", "kind": "agentMessage",
             "text": "fork here", "turnId": "turn-1", "status": "completed"},
            {"itemId": "msg-fork-dup", "kind": "agentMessage",
             "text": "fork here", "turnId": "turn-2", "status": "completed"},
            {"itemId": "msg-fork-2", "kind": "agentMessage",
             "text": "later answer", "turnId": "turn-2", "status": "completed"},
            {"itemId": "shell-no-turn", "kind": "userShell",
             "commandText": "git status", "turnId": None,
             "status": "completed"},
        ]
        return {"session": session_obj(read_params.get("sessionId")),
                "history": {"mode": "inline", "items": items,
                            "snapshot": None},
                "viewCursor": "cur-read", "pendingRequests": []}
    if method == "approval/decide":
        log_input(msg.get("params", {}))
        if SCENARIO == "status_flags":
            notify("session/statusChanged", {
                "sessionId": MSP_SID, "status": "paused",
                "attention": ["futureAttention"], "viewCursor": None})
            notify("view/gap", {"sessionId": MSP_SID})
            notify("session/statusChanged", {
                "sessionId": MSP_SID, "status": "idle",
                "viewCursor": "cur-status-2"})
        return {"status": "accepted"}
    if method == "session/fork":
        params = msg.get("params", {})
        log_input(params)
        ACTIVE_SESSION[0] = "msp-sess-forked"
        forked = session_obj()
        forked["forkedFrom"] = {
            "sessionId": params.get("sessionId", MSP_SID),
            "commandId": params.get("commandId", ""),
            "cutCursor": "opaque-cut",
            "cutExplicit": "cutPoint" in params,
        }
        if SCENARIO.startswith("fork_history"):
            FORK_ITEMS[:] = [
                {"itemId": "msg-fork", "kind": "agentMessage", "text": "fork here",
                 "turnId": "turn-1", "status": "completed"},
            ]
            if params.get("cutPoint", {}).get("lastTurnId") != "turn-1":
                FORK_ITEMS.append({"itemId": "msg-fork-dup", "kind": "agentMessage",
                                   "text": "second turn", "turnId": "turn-2", "status": "completed"})
            if SCENARIO == "fork_history_inline":
                history = {"mode": "inline", "items": FORK_ITEMS, "snapshot": None}
            elif SCENARIO == "fork_history_snapshot":
                history = {"mode": "snapshot", "items": None, "snapshot": {
                    "schemaVersion": 1, "viewCursor": "fork-start",
                    "state": {"items": [dict(FORK_ITEMS[0], text="partial")]}}}
            else:
                history = {"mode": "none", "items": None, "snapshot": None}
            # A repeated live completion at the replay boundary must not double render.
            notify("item/completed", {"sessionId": ACTIVE_SESSION[0],
                                      "viewCursor": "fork-head", "item": FORK_ITEMS[-1]})
            return {"session": forked, "history": history,
                    "viewCursor": "fork-head", "pendingRequests": []}
        return {"session": forked,
                "history": {"mode": "inline", "items": [], "snapshot": None},
                "viewCursor": "cur-f1", "pendingRequests": []}
    if method == "session/userShell":
        params = msg.get("params", {})
        log_input(params)
        item = {"itemId": "shell-explicit", "kind": "userShell", "turnId": None,
                "commandId": params["commandId"], "commandText": params["commandText"],
                "status": "inProgress", "revision": 1}
        notify("item/started", {"sessionId": params["sessionId"], "item": item})
        if SCENARIO == "user_shell_restart":
            CRASH_AFTER_ACK[0] = True
            return {"commandId": params["commandId"], "status": "accepted"}
        notify("item/completed", {"sessionId": params["sessionId"], "item": {
            **item, "status": "completed", "revision": 2, "exitSignal": 15,
            "visibleOutput": "shell stopped"}})
        return {"commandId": params["commandId"], "status": "accepted"}
    if method == "session/rename":
        params = msg.get("params", {})
        log_input(params)
        name = params.get("name", "")
        notify("session/nameChanged", {
            "sessionId": params.get("sessionId", MSP_SID), "name": name,
            "viewCursor": "cur-rename-1", "sourceRange": {"start": 1, "end": 1},
        })
        return {"commandId": params.get("commandId", ""), "status": "accepted",
                "name": name}
    if method in ("goal/set", "goal/edit", "goal/pause", "goal/resume",
                    "goal/clear"):
        params = msg.get("params", {})
        log_input(params)
        return on_goal_command(method, params)
    if method == "session/compact":
        log_input(msg.get("params", {}))
        if SCENARIO == "compact_noop":
            return {"commandId": msg["params"].get("commandId", ""),
                    "status": "noop", "reason": "no_compactable_history"}
        notify("item/completed", {"sessionId": MSP_SID, "item": {
            "itemId": "it-c1", "kind": "compaction", "status": "completed",
            "revision": 1, "outcome": "compacted",
            "tokensBefore": 12000, "tokensAfter": 8000,
            "trigger": "manual"}, "viewCursor": "cur-c1"})
        return {"commandId": msg["params"].get("commandId", ""),
                "status": "accepted"}
    if method == "task/stop" and SCENARIO == "async_task_stop":
        params = msg.get("params", {})
        notify("item/updated", {"sessionId": MSP_SID, "item": {
            "itemId": "it-bg-stop", "kind": "toolCall", "callId": "call-bg-stop",
            "status": "cancelled", "revision": 2, "tool": "workspace-shell",
            "args": {"command": "npm watch"}, "background": True,
            "backgroundInitiator": "user"}})
        return {"commandId": params.get("commandId", ""),
                "status": "accepted", "taskId": params.get("taskId", "")}
    if method == "task/stopAll" and SCENARIO == "async_task_stop_all":
        for item_id, call_id, command in [
            ("it-bg-all-1", "call-bg-all-1", "npm watch"),
            ("it-bg-all-2", "call-bg-all-2", "cargo watch"),
        ]:
            notify("item/completed", {"sessionId": MSP_SID, "item": {
                "itemId": item_id, "kind": "toolCall", "callId": call_id,
                "status": "cancelled", "revision": 2, "tool": "workspace-shell",
                "args": {"command": command}, "background": True,
                "failureReason": "stopped by user"}})
        params = msg.get("params", {})
        return {"commandId": params.get("commandId", ""), "status": "accepted"}
    if method == "userInput/answer":
        log_input(msg.get("params", {}))
        return {}
    if method in ("approval/decide", "userInput/clarify"):
        log_input(msg.get("params", {}))
        return {}
    if method == "turn/steer":
        params = msg.get("params", {})
        log_input(params)
        return {
            "commandId": params.get("commandId", ""),
            "status": "accepted",
            "turnId": params.get("expectedTurnId", ""),
        }
    if method.startswith("subagent/"):
        params = msg.get("params", {})
        log_input(params)
        return {"commandId": params.get("commandId", ""), "status": "accepted"}
    if method == "workflow/childControl":
        params = msg.get("params", {})
        log_input(params)
        # A retry starts the next attempt; with FAKE_WORKFLOW_SILENT_RETRY the
        # new attempt is not announced, so the adapter's next control is stale
        # (rejected in the dispatch loop).
        if params.get("action") == "retry":
            WORKFLOW_CHILD_ATTEMPT[0] += 1
            if not os.environ.get("FAKE_WORKFLOW_SILENT_RETRY"):
                notify("item/updated", {
                    "sessionId": MSP_SID, "turnId": WORKFLOW_TURN[0], "item": {
                        "itemId": "it-wf-control", "kind": "workflow",
                        "status": "inProgress", "revision": 2 + WORKFLOW_CHILD_ATTEMPT[0],
                        "workflowRunId": "wfr-control", "entryId": "triage-batch",
                        "scriptId": "triage@sha256:aa10",
                        "triggerSource": "modelProposal",
                        "children": [{"childId": "c1",
                                      "attempt": WORKFLOW_CHILD_ATTEMPT[0],
                                      "status": "started", "phase": "triage",
                                      "label": "triage issue #1"}]}})
        return {"commandId": params.get("commandId", ""), "status": "accepted"}
    if method == "workflow/cancel":
        params = msg.get("params", {})
        log_input(params)
        if SCENARIO == "workflow_control":
            base = {"sessionId": MSP_SID, "turnId": WORKFLOW_TURN[0]}
            item = {
                "itemId": "it-wf-control", "kind": "workflow",
                "status": "cancelled", "revision": 3,
                "workflowRunId": "wfr-control", "entryId": "triage-batch",
                "scriptId": "triage@sha256:aa10",
                "triggerSource": "modelProposal",
                "children": [{"childId": "c1", "attempt": 1,
                              "status": "cancelled", "phase": "triage",
                              "label": "triage issue #1", "terminal": "cancelled"}],
                "message": "Workflow triage-batch cancelled: 0/1 children succeeded",
            }
            notify("item/updated", {**base, "item": item})
            notify("item/completed", {**base, "item": item})
            notify("turn/completed", {**base, "terminal": "cancelled"})
        return {"commandId": params.get("commandId", ""), "status": "accepted"}
    if method in ("turn/cancel", "turn/interrupt"):
        # Like the real host: a cancelled or interrupted turn still reports
        # its terminal. Keep the request in the input log so tests can verify
        # the exact turn and interrupt posture.
        params = msg.get("params", {})
        log_input(params)
        if GOAL_TURN[0] and params.get("turnId") == GOAL_TURN[0]:
            # Interrupting a goal turn pauses the goal (host safety rule).
            GOAL_TURN[0] = ""
            goal_changed("paused")
        notify("turn/completed", {"sessionId": MSP_SID,
                                  "turnId": params.get("turnId", ""),
                                  "terminal": "cancelled"})
        return {}
    return {}


def scenario_after_restart():
    """host_exit is a one-shot: the first process creates the marker and
    crashes; the replacement process sees the marker and behaves sanely."""
    marker = os.environ.get("FAKE_RESTART_MARKER", "")
    if SCENARIO in ("host_exit", "host_exit_quiet", "host_exit_relaunch_unavailable", "close_stdin", "stdout_close_stays_alive", "user_shell_restart") and marker:
        if os.path.exists(marker):
            if SCENARIO == "user_shell_restart":
                return "user_shell_recovered"
            if SCENARIO == "host_exit_relaunch_unavailable":
                return "support_exit"
            return "happy"
        with open(marker, "w") as f:
            f.write("crashed")
    return SCENARIO


SCENARIO = scenario_after_restart()


def main():
    if os.environ.get("FAKE_CHECK_HOST_CONFIG") == "1" and sys.argv[1:2] == ["serve"]:
        root = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
        with open(os.path.join(root, "muse", "settings.json")) as source:
            settings = json.load(source)
        with open(LOG + ".config", "a") as log:
            log.write(json.dumps({"root": root, "settings": settings, "args": sys.argv[1:]}) + "\n")
        if settings.get("permissions", {}).get("default_profile") == ":auto-review":
            os.environ["FAKE_START_ERROR"] = "profile"
    if SCENARIO == "support_exit":
        message = os.environ.get("FAKE_HOST_STDERR", "serve diagnostic")
        sys.stderr.write(message + ("" if message.endswith("\n") else "\n"))
        sys.stderr.flush()
        os._exit(
            int(
                os.environ.get(
                    "FAKE_RESTART_EXIT_CODE",
                    os.environ.get("FAKE_HOST_EXIT_CODE", "5"),
                )
            )
        )
    pid_path = os.environ.get("FAKE_PID", "")
    if pid_path:
        with open(pid_path, "a") as f:
            f.write(str(os.getpid()))
            f.write("\n")
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = msg.get("method", "")
        ident = msg.get("id")
        log_method(method or "(response)")
        # Every client->host request frame, method-tagged, for conformance
        # validation against the vendored schema bundle.
        if method and LOG:
            with open(os.environ.get("FAKE_FRAMES", ""), "a") as f:
                f.write(json.dumps({"method": method,
                                    "params": msg.get("params", {})}) + "\n")
        if not method and ident == "srv-77":
            # The adapter's reply to the fixture's unknown server request.
            log_method("unknown-request-reply:" + json.dumps(msg))
            continue
        if SCENARIO == "shutdown_pending_command" and method == "session/list":
            log_method("command-stalled")
            time.sleep(60)
            continue
        if method == "initialized":
            if SCENARIO == "malformed_msp":
                sys.stdout.buffer.write(b'{"method":"item/delta","params":{"text":"\xff"}}\n')
                sys.stdout.buffer.flush()
                for invalid in ('{"method":"item/delta","params":', '{"id":99,"method":"approval/request",}', r'{"method":"item/delta","params":{"text":"\u+D4A"}}'):
                    sys.stdout.write(invalid + "\n")
                sys.stdout.flush()
            if SCENARIO == "close_stdin":
                os.close(0)
            if SCENARIO == "unknown_request":
                send({"jsonrpc": "2.0", "id": "srv-77",
                      "method": "future/request",
                      "params": {"sessionId": MSP_SID, "novel": True}})
            continue
        if method:
            if ident is not None:
                if method == "initialize" and SCENARIO == "stderr_flood":
                    sys.stderr.buffer.write(b"fixture stderr flood\n" * 32768)
                    sys.stderr.flush()
                if (os.environ.get("FAKE_DELAY_METHOD", "") == method
                        and os.environ.get("FAKE_DELAY_MS", "")):
                    time.sleep(int(os.environ["FAKE_DELAY_MS"]) / 1000.0)
                if (method == "model/list" and SCENARIO == "catalog_refresh_failure"
                        and CATALOG_READS[0] == 2):
                    CATALOG_READS[0] += 1
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32603, "message": "catalog unavailable"}})
                    continue
                if method == os.environ.get("FAKE_ERROR_METHOD"):
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32603,
                                    "message": os.environ["FAKE_ERROR_MESSAGE"]}})
                    continue
                if (method == "item/readOutput"
                        and SCENARIO == "tool_output_unavailable"):
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32041,
                                    "message": "stored output unavailable",
                                    "data": {"kind": "outputUnavailable",
                                             "availability": "missing",
                                             "itemId": "it-stored",
                                             "outputRef": "out-1"}}})
                    continue
                mcp_servers = ((msg.get("params") or {}).get("config") or {}).get("mcpServers")
                if method in ("session/start", "session/resume") and mcp_servers:
                    if not SESSION_MCP[0]:
                        # Live 1.4.1 refusal without the grant.
                        send({"jsonrpc": "2.0", "id": ident,
                              "error": {"code": -32010,
                                        "message": "session MCP configuration requires the sessionMcp capability",
                                        "data": {"kind": "capabilityRequired",
                                                 "capability": "sessionMcp",
                                                 "retryable": False}}})
                        continue
                    if (method == "session/resume" and not MCP_CONFLICTED[0]
                            and os.environ.get("FAKE_MCP_CONFLICT") == "1"):
                        MCP_CONFLICTED[0] = True
                        command = msg["params"].get("commandId", "")
                        send({"jsonrpc": "2.0", "id": ident,
                              "error": {"code": -32030,
                                        "message": "command " + command + " rejected: session MCP configuration conflicts with the loaded session runtime",
                                        "data": {"kind": "commandRejected",
                                                 "commandId": command,
                                                 "reason": "session_configuration_conflict",
                                                 "retryable": False}}})
                        continue
                if method == "session/compact" and SCENARIO in (
                        "compact_unavailable", "compact_rejected"):
                    # compaction_unavailable mirrors live 1.3-1.4 hosts on a
                    # session too short to compact; the other is any refusal.
                    log_input(msg.get("params", {}))
                    command = msg["params"].get("commandId", "")
                    reason = ("compaction_unavailable" if SCENARIO == "compact_unavailable"
                              else "turn_in_progress")
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32030,
                                    "message": "session/compact command " + command + " rejected: " + reason,
                                    "data": {"kind": "commandRejected",
                                             "commandId": command,
                                             "reason": reason,
                                             "retryable": False}}})
                    continue
                if (method == "session/start"
                        and os.environ.get("FAKE_START_ERROR", "") == "profile"):
                    # Mirrors the live 1.2.1 refusal when the user's default
                    # permission profile needs an unavailable reviewer.
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32603,
                                    "message": "internal error: compose session permission profile: permission profile ':auto-review' cannot be used: the automated reviewer is unavailable on this host",
                                    "data": {"kind": "internal"}}})
                    continue
                if method == "turn/start" and SCENARIO == "host_exit_before_ack":
                    message = os.environ.get("FAKE_HOST_STDERR", "")
                    if message:
                        sys.stderr.write(message + ("" if message.endswith("\n") else "\n"))
                        sys.stderr.flush()
                    os._exit(int(os.environ.get("FAKE_HOST_EXIT_CODE", "5")))
                if method == "account/read" and SCENARIO.startswith("account_"):
                    # Live 1.4.0: account/* exists only behind experimentalApi.
                    if not EXPERIMENTAL_API[0]:
                        send({"jsonrpc": "2.0", "id": ident,
                              "error": {"code": -32601,
                                        "message": "method not found: account/read",
                                        "data": {"kind": "methodNotFound"}}})
                        continue
                    state = {
                        "account_logged_out": {"state": "loggedOut",
                                               "credentialRequired": True},
                        "account_keyless": {"state": "loggedOut",
                                            "credentialRequired": False},
                        "account_logged_in": {"state": "accountLogin",
                                              "credentialRequired": True},
                    }[SCENARIO]
                    send({"jsonrpc": "2.0", "id": ident, "result": state})
                    continue
                if (method == "session/setReasoningEffort"
                        and SCENARIO == "reasoning_legacy"):
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32601,
                                    "message": "method not found: session/setReasoningEffort",
                                    "data": {"kind": "methodNotFound"}}})
                    continue
                if (method == "session/delete"
                        and os.environ.get("FAKE_DELETE_REJECT")):
                    log_input(msg.get("params", {}))
                    reason = os.environ["FAKE_DELETE_REJECT"]
                    if reason == "sessionNotFound":
                        error = {"code": -32020, "message": "session not found",
                                 "data": {"kind": "sessionNotFound"}}
                    else:
                        error = {"code": -32030, "message": reason,
                                 "data": {"kind": "commandRejected",
                                          "reason": reason, "retryable": False}}
                    send({"jsonrpc": "2.0", "id": ident, "error": error})
                    continue
                if (method == "session/list" and os.environ.get("FAKE_LIST_REJECT_CURSOR") == "1"
                        and msg.get("params", {}).get("cursor")):
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32602, "message": "invalid cursor",
                                    "data": {"kind": "invalidParams"}}})
                    continue
                if (method == "session/resume"
                        and os.environ.get("FAKE_RESUME_NOT_FOUND") == "1"):
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32020, "message": "session not found",
                                    "data": {"kind": "sessionNotFound",
                                             "sessionId": msg.get("params", {}).get("sessionId")}}})
                    continue
                if method in ("session/start", "turn/start", "turn/steer"):
                    params = msg.get("params", {})
                    roots = params.get("workspaceRoots")
                    if roots is not None:
                        problem = validate_workspace_roots(
                            roots, params.get("workspaceRoot", ACTIVE_WORKSPACE[0]))
                        if problem:
                            log_input(params)
                            send({"jsonrpc": "2.0", "id": ident,
                                  "error": {"code": -32602,
                                            "message": "Invalid params: " + problem,
                                            "data": {"kind": "invalidParams"}}})
                            continue
                if method == "feedback/submit":
                    params = msg.get("params", {})
                    error = None
                    if not FEEDBACK[0]:
                        error = {"code": -32010,
                                 "message": "feedback/submit requires the feedback capability",
                                 "data": {"kind": "capabilityRequired",
                                          "capability": "feedback", "retryable": False}}
                    elif (params.get("classification") == "bug"
                          and not (params.get("note") or "").strip()):
                        error = {"code": -32602,
                                 "message": "Invalid params: note must be non-empty for bug",
                                 "data": {"kind": "invalidParams"}}
                    elif os.environ.get("FAKE_FEEDBACK_HOST_ERROR") == "1":
                        error = {"code": -32603, "message": "feedback upload failed",
                                 "data": {"kind": "internal"}}
                    if error:
                        log_input(params)
                        send({"jsonrpc": "2.0", "id": ident, "error": error})
                        continue
                if (method == "workflow/childControl"
                        and msg.get("params", {}).get("attempt")
                        != WORKFLOW_CHILD_ATTEMPT[0]):
                    # The host keys child control by (childId, attempt): a
                    # stale attempt is rejected, never re-keyed.
                    log_input(msg.get("params", {}))
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32030,
                                    "message": "stale_attempt: re-read the workflow item",
                                    "data": {"kind": "commandRejected",
                                             "reason": "stale_attempt"}}})
                    continue
                if (SCENARIO == "skill_not_found"
                        and method in ("turn/start", "turn/steer")):
                    log_input(msg.get("params", {}))
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32032,
                                    "message": "skill selector was not found",
                                    "data": {"kind": "skillNotFound",
                                             "selector": "stale"}}})
                    continue
                send({"jsonrpc": "2.0", "id": ident,
                      "result": result_for(method, msg)})
                if REVIEW_HOST and method == "turn/start":
                    text = os.environ.get(
                        "FAKE_REVIEW_TEXT",
                        '{"outcome":"allow","rationale":"routine action"}')
                    notify("item/delta", {
                        "sessionId": REVIEW_SID, "itemId": "review-msg",
                        "field": "text", "delta": text})
                    notify("item/completed", {
                        "sessionId": REVIEW_SID,
                        "item": {"itemId": "review-msg", "kind": "agentMessage",
                                 "status": "completed", "text": text}})
                    notify("turn/completed", {
                        "sessionId": REVIEW_SID, "turnId": "review-turn-1",
                        "terminal": "completed"})
                    continue
                if SCENARIO in ("session_list_stream", "session_list_stream_denied") \
                        and method == "session/start":
                    root = msg.get("params", {}).get("workspaceRoot", "/tmp/fake-ws")
                    log_method("session/listChanged-sent")
                    send({"jsonrpc": "2.0", "method": "session/started",
                          "params": {"session": session_obj(workspace_root=root)}})
                    changed = session_obj(workspace_root=root)
                    changed["title"] = ("Renamed elsewhere"
                                        if SCENARIO == "session_list_stream"
                                        else "Should be ignored")
                    if os.environ.get("FAKE_STREAM_METADATA") == "1":
                        changed.update({"name": "Streamed name", "title": "Lower priority",
                                        "branch": {"branch": "live-branch"},
                                        "attention": ["inputPending"]})
                    send({"jsonrpc": "2.0", "method": "session/listChanged",
                          "params": {"session": changed}})
                if SCENARIO == "session_list_stream" and method == "session/list" \
                        and os.environ.get("FAKE_STREAM_METADATA") == "1":
                    send({"jsonrpc": "2.0", "method": "session/listChanged",
                          "params": {"session": {"sessionId": MSP_SID,
                                                 "workspaceRoot": ACTIVE_WORKSPACE[0]}}})
                if SCENARIO == "session_list_stream" and method == "session/list" \
                        and os.environ.get("FAKE_STREAM_METADATA") != "1":
                    log_method("session/closed-sent")
                    # Optional delay between the marker and the notification,
                    # so tests cannot synchronize on the marker by luck.
                    if os.environ.get("FAKE_CLOSE_DELAY_MS"):
                        time.sleep(int(os.environ["FAKE_CLOSE_DELAY_MS"]) / 1000.0)
                    # An idle unload, shaped as SessionClosedParams: the
                    # session is notLoaded, not deleted.
                    send({"jsonrpc": "2.0", "method": "session/closed",
                          "params": {"reason": "idle", "sessionId": MSP_SID,
                                     "viewCursor": "cur-closed"}})
                if method == "session/setReasoningEffort":
                    notify("session/reasoningEffortChanged", {
                        "sessionId": msg.get("params", {}).get("sessionId", MSP_SID),
                        "reasoningEffort": msg.get("params", {}).get("reasoningEffort", ""),
                        "source": "user",
                        "viewCursor": "cur-reasoning-1",
                        "sourceRange": {"start": 1, "end": 1},
                    })
                if method == "session/setModel":
                    model = ((msg.get("params", {}).get("model") or {})
                             .get("modelId", ""))
                    if model:
                        notify("session/modelChanged", {
                            "sessionId": msg.get("params", {}).get("sessionId", MSP_SID),
                            "modelId": model,
                        })
                if SCENARIO == "rename_live" and method == "session/start":
                    notify("session/nameChanged", {
                        "sessionId": MSP_SID,
                        "name": "Renamed elsewhere",
                        "viewCursor": "cur-1",
                        "sourceRange": {"start": 1, "end": 1},
                    })
                if method == "session/list" and SCENARIO == "pipe_stall":
                    log_method("pipe-stall-start")
                    # Stay stalled well past the adapter's 5s write timeout,
                    # however late a slow runner starts that write; the
                    # adapter kills this process when the timeout fires.
                    time.sleep(60)
                if method == "model/list" and SCENARIO == "stdout_close_stays_alive":
                    os.close(1)
                    time.sleep(1.0)
                if method == "session/delete":
                    # The ack is out; now report the outcome as the host's
                    # later terminal notification.
                    params = msg.get("params", {})
                    sid = params.get("sessionId", "")
                    if os.environ.get("FAKE_DELETE_EXIT") == "1":
                        sys.stdout.flush()
                        os._exit(int(os.environ.get("FAKE_DELETE_EXIT_CODE", "1")))
                    if os.environ.get("FAKE_DELETE_DELAY_MS"):
                        time.sleep(int(os.environ["FAKE_DELETE_DELAY_MS"]) / 1000.0)
                    if os.environ.get("FAKE_DELETE_OUTCOME", "completed") == "failed":
                        notify("session/deleteCompleted", {
                            "commandId": params.get("commandId", ""),
                            "sessionId": sid,
                            "outcome": "failed",
                            "reason": os.environ.get("FAKE_DELETE_REASON",
                                                     "ownershipUnavailable"),
                            "physicalChange": os.environ.get("FAKE_DELETE_PHYSICAL",
                                                             "none"),
                        })
                    else:
                        DELETED_SESSIONS.append(sid)
                        notify("session/deleteCompleted", {
                            "commandId": params.get("commandId", ""),
                            "sessionId": sid,
                            "outcome": "completed",
                        })
                if CRASH_AFTER_ACK[0]:
                    sys.stdout.flush()
                    message = os.environ.get("FAKE_HOST_STDERR", "")
                    if message:
                        sys.stderr.write(message + ("" if message.endswith("\n") else "\n"))
                        sys.stderr.flush()
                    default_code = "1" if SCENARIO in ("host_exit", "host_exit_quiet", "user_shell_restart", "host_crash_loop") else "0"
                    os._exit(int(os.environ.get("FAKE_HOST_EXIT_CODE", default_code)))
                if SCENARIO == "skills_changed" and method == "session/start":
                    notify("skill/changed", {"sessionId": MSP_SID})
                if (SCENARIO == "mcp_oauth_completed" and method == "session/start"
                        and EXPERIMENTAL_API[0]):
                    # Delivered to every experimental connection, initiator
                    # or not; never carries the authorization URL or keys.
                    notify("mcpServer/oauthLoginCompleted", {
                        "outcome": "granted", "server": "fixture-mcp",
                        "message": "fixture login detail"})
                if SCENARIO == "questions_resume" and method == "session/resume":
                    # MSP reissues pending requests after the resume response.
                    send({"jsonrpc": "2.0", "id": 9100 + ident,
                          "method": "userInput/request", "params": question_params()})
                    if msg["params"].get("history") == "snapshot":
                        # A stream barrier: both reissues precede this item.
                        notify("item/completed", {"sessionId": MSP_SID, "item": {
                            "itemId": "reissue-barrier", "kind": "agentMessage",
                            "status": "completed", "text": "resume questions delivered"}})
                if SCENARIO == "view_subscribe_gap" and method == "view/subscribe":
                    replay = {"sessionId": MSP_SID, "viewCursor": "cur-1",
                              "item": {"itemId": "view-gap-item",
                                       "kind": "agentMessage", "status": "completed",
                                       "text": "replayed after cursor"}}
                    # A duplicate delivery must not duplicate the editor event.
                    notify("item/completed", replay)
                    notify("item/completed", replay)
                    usage = {"sessionId": MSP_SID, "usedTokens": 42,
                             "windowTokens": 100, "pressure": "warning",
                             "viewCursor": "cur-2"}
                    notify("session/contextUsage", usage)
                    notify("session/contextUsage", usage)

    if SCENARIO == "shutdown_stubborn":
        log_method("shutdown-stubborn")
        time.sleep(60)
    if SCENARIO == "shutdown_flush":
        time.sleep(0.6)
        log_method("shutdown-flushed")


main()
