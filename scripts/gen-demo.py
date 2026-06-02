#!/usr/bin/env python3
"""Generate a reproducible demo corpus for csess screenshots.

Writes Claude (`<root>/claude/<encoded-cwd>/<uuid>.jsonl`) and Codex
(`<root>/codex/YYYY/MM/DD/rollout-*-<uuid>.jsonl`) sessions, then back-dates
each file's mtime so the relative-time column is stable. Pair with `CSESS_NOW`
(printed at the end) so the TUI renders identical frames every run.

    python3 scripts/gen-demo.py [target_dir]   # default /tmp/csess-demo
"""
import json
import os
import shutil
import sys

NOW = 1893456000  # fixed "now" — must match CSESS_NOW in the VHS tape
HOME = "/Users/casey"  # demo home; the tree root (~) hangs off this

# (offset_seconds_ago, source, cwd, uuid, title, turns)
# turns: list of (role, blocks). Claude blocks: ("text"|"think"|"tool", payload).
# Codex blocks: ("text"|"think", payload). role: "user"|"assistant".
JWT_PLAN = """## Plan

I'll add JWT auth in three steps:

- **middleware** — verify the `Authorization` header
- **login route** — issue a signed token
- **guard** — reject expired tokens

Here's the middleware:

```python
def require_auth(handler):
    def wrapper(req):
        token = req.headers.get("Authorization", "").removeprefix("Bearer ")
        if not verify(token):
            return Response(401, "unauthorized")
        return handler(req)
    return wrapper
```

Next I'll wire it into the router."""

SESSIONS = [
    # ---- newest first ----
    (240, "claude", f"{HOME}/work/web-app", "a1f00001-0000-7000-8000-000000000001",
     "Add JWT auth to the API endpoints", [
        ("user", [("text", "Add JWT auth to the API endpoints")]),
        ("assistant", [
            ("think", "First let me see how the routes are wired, then add a middleware layer."),
            ("tool", ("Read", f"{HOME}/work/web-app/api/router.py")),
            ("text", JWT_PLAN),
        ]),
     ]),
    (540, "codex", f"{HOME}/work/cli-tool", "c0de0002-0000-7000-8000-000000000002",
     "Refactor the argument parser to use subcommands", [
        ("user", [("text", "Refactor the argument parser to use subcommands")]),
        ("assistant", [
            ("think", "The current parser is one flat flag switch; subcommands need their own scopes."),
            ("text", "I'll split it into `build`, `run`, and `index` subcommands, each with its own flag set, and keep a shared `--verbose`. Wiring the dispatch table now."),
        ]),
     ]),
    (840, "claude", f"{HOME}/work/web-app/web", "a1f00003-0000-7000-8000-000000000003",
     "Dark mode toggle flickers on first paint", [
        ("user", [("text", "Dark mode toggle flickers on first paint")]),
        ("assistant", [
            ("text", "That flicker is a hydration mismatch: the server renders the light theme, then the client reads `localStorage` after first paint and swaps. Inline a tiny script in `<head>` that sets the `data-theme` attribute **before** the body renders, so the first paint already matches."),
        ]),
     ]),
    (1320, "codex", f"{HOME}/work/notes", "c0de0004-0000-7000-8000-000000000004",
     "Why is the /search endpoint so slow?", [
        ("user", [("text", "Why is the /search endpoint so slow?")]),
        ("assistant", [
            ("text", "The query does a full table scan on every keystroke — there's no index on `documents(body)`. Add a trigram index and debounce the request by 150ms. That drops p95 from 800ms to ~40ms."),
        ]),
     ]),
    (1860, "claude", f"{HOME}/work/web-app/api", "a1f00005-0000-7000-8000-000000000005",
     "Design the pagination contract for the list endpoints", [
        ("user", [("text", "Design the pagination contract for the list endpoints")]),
        ("assistant", [
            ("text", "Use **cursor-based** pagination, not offset:\n\n- response carries `next_cursor` (opaque, base64 of `(sort_key, id)`)\n- request takes `?after=<cursor>&limit=<n>` with `limit` capped at 100\n- stable under inserts, unlike `OFFSET`\n\nI'll add the cursor codec next."),
        ]),
     ]),
    (2820, "codex", f"{HOME}/sandbox/scratch", "c0de0006-0000-7000-8000-000000000006",
     "Quick test of the new embedding model", [
        ("user", [("text", "Quick test of the new embedding model")]),
        ("assistant", [
            ("text", "Ran 500 sample queries through it. Recall@10 is up from 0.71 to 0.83, latency about the same (12ms/query batched). Worth promoting to staging."),
        ]),
     ]),
    (3700, "claude", f"{HOME}/work/parser", "a1f00007-0000-7000-8000-000000000007",
     "Fix the lexer on escaped quotes inside strings", [
        ("user", [("text", "Fix the lexer on escaped quotes inside strings")]),
        ("assistant", [
            ("think", "The scanner ends the string on the first quote, ignoring the preceding backslash."),
            ("text", "The bug is in `scan_string`: it stops at any `\"` without checking for a preceding `\\`. Track an `escaped` flag and only close the string when `!escaped`. Added a fixture for `\"a\\\"b\"`."),
        ]),
     ]),
    (7200, "codex", f"{HOME}/work/web-app/api", "c0de0008-0000-7000-8000-000000000008",
     "Add rate limiting to the gateway", [
        ("user", [("text", "Add rate limiting to the gateway")]),
        ("assistant", [
            ("text", "Token-bucket per API key, 60 req/min with a burst of 20, stored in Redis with a sliding TTL. Returns `429` plus a `Retry-After` header when the bucket is empty."),
        ]),
     ]),
    (10800, "claude", f"{HOME}/work/notes", "a1f00009-0000-7000-8000-000000000009",
     "Summarize the architecture review from yesterday", [
        ("user", [("text", "Summarize the architecture review from yesterday")]),
        ("assistant", [
            ("text", "Three decisions landed:\n\n- split the monolith **read path** behind a cache, leave writes in place\n- adopt cursor pagination everywhere (deprecate offset by Q3)\n- move background jobs to a queue instead of cron\n\nOpen question: do we shard by tenant now or later?"),
        ]),
     ]),
    (75600, "claude", f"{HOME}/work/web-app", "a1f0000a-0000-7000-8000-00000000000a",
     "Set up the CI pipeline with cargo test and clippy", [
        ("user", [("text", "Set up the CI pipeline with cargo test and clippy")]),
        ("assistant", [
            ("text", "Added a GitHub Actions workflow: `cargo fmt --check`, `cargo clippy -- -D warnings`, then `cargo test --all`. Caches `~/.cargo` and `target/` keyed on `Cargo.lock`."),
        ]),
     ]),
    (180000, "codex", f"{HOME}/work/cli-tool", "c0de000b-0000-7000-8000-00000000000b",
     "Add shell completion for bash and zsh", [
        ("user", [("text", "Add shell completion for bash and zsh")]),
        ("assistant", [
            ("text", "Generated completions with `clap_complete` for bash, zsh, and fish, wired to a hidden `completions <shell>` subcommand so packagers can emit them at build time."),
        ]),
     ]),
]


def encode_cwd(cwd: str) -> str:
    """Claude's project-dir encoding: every '/' becomes '-'."""
    return cwd.replace("/", "-")


def claude_lines(cwd, uuid, turns):
    out = []
    for role, blocks in turns:
        if role == "user":
            text = blocks[0][1]
            out.append({"type": "user", "cwd": cwd, "sessionId": uuid,
                        "message": {"content": text}})
        else:
            content = []
            for kind, payload in blocks:
                if kind == "text":
                    content.append({"type": "text", "text": payload})
                elif kind == "think":
                    content.append({"type": "thinking", "thinking": payload})
                elif kind == "tool":
                    name, fp = payload
                    content.append({"type": "tool_use", "name": name,
                                    "input": {"file_path": fp}})
            out.append({"type": "assistant", "cwd": cwd, "sessionId": uuid,
                        "message": {"content": content}})
    return out


def codex_lines(cwd, uuid, turns):
    out = [{"type": "session_meta", "payload": {"id": uuid, "cwd": cwd}}]
    for role, blocks in turns:
        if role == "user":
            out.append({"type": "event_msg",
                        "payload": {"type": "user_message", "message": blocks[0][1]}})
        else:
            for kind, payload in blocks:
                ptype = "agent_reasoning" if kind == "think" else "agent_message"
                out.append({"type": "event_msg",
                            "payload": {"type": ptype, "message": payload}})
    return out


def write_jsonl(path, lines, mtime):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        for obj in lines:
            f.write(json.dumps(obj, ensure_ascii=False) + "\n")
    os.utime(path, (mtime, mtime))


def main():
    target = sys.argv[1] if len(sys.argv) > 1 else "/tmp/csess-demo"
    claude_root = os.path.join(target, "claude")
    codex_root = os.path.join(target, "codex")
    shutil.rmtree(target, ignore_errors=True)
    os.makedirs(claude_root, exist_ok=True)
    os.makedirs(codex_root, exist_ok=True)

    n_claude = n_codex = 0
    for offset, source, cwd, uuid, _title, turns in SESSIONS:
        mtime = NOW - offset
        if source == "claude":
            path = os.path.join(claude_root, encode_cwd(cwd), f"{uuid}.jsonl")
            write_jsonl(path, claude_lines(cwd, uuid, turns), mtime)
            n_claude += 1
        else:
            day = os.path.join(codex_root, "2030", "01", "15")
            path = os.path.join(day, f"rollout-2030-01-15T10-00-00-{uuid}.jsonl")
            write_jsonl(path, codex_lines(cwd, uuid, turns), mtime)
            n_codex += 1

    print(f"wrote {n_claude} Claude + {n_codex} Codex sessions to {target}")
    print("env for csess / VHS:")
    print(f"  CSESS_CLAUDE_ROOT={claude_root}")
    print(f"  CSESS_CODEX_ROOT={codex_root}")
    print(f"  CSESS_DB={os.path.join(target, 'index.db')}")
    print(f"  CSESS_NOW={NOW}")
    print(f"  HOME={HOME}")


if __name__ == "__main__":
    main()
