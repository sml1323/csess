# csess screenshots — regenerate with:  vhs scripts/demo.tape
# (run scripts/gen-demo.py first; CSESS_NOW below must match the generator)
Output "/tmp/csess-demo/_recording.gif"

Set Shell "bash"
Set FontSize 26
Set Width 2000
Set Height 1150
Set Padding 24
Set Margin 28
Set MarginFill "#0d0f14"
Set BorderRadius 12
Set WindowBar Colorful
Set CursorBlink false

Env CSESS_CLAUDE_ROOT "/tmp/csess-demo/claude"
Env CSESS_CODEX_ROOT "/tmp/csess-demo/codex"
Env CSESS_DB "/tmp/csess-demo/index.db"
Env CSESS_NOW "1893456000"
Env HOME "/Users/casey"

# launch (hidden so the screenshot is pure TUI, no shell prompt)
# stty -ixon: stop the pty eating Ctrl+S as XOFF flow-control before it reaches csess
Hide
Type "stty -ixon"
Enter
Sleep 200ms
Type "./target/release/csess"
Enter
Sleep 1.6s
Show
Sleep 400ms

# 1. hero — flat list (dual-tone c/ⓒ) + rich preview
Screenshot "docs/img/list-preview.png"

# 2. body search — narrow to one session
# trailing space forces the final 'h' to repaint (VHS drops the last keystroke's
# frame); split_whitespace ignores it so the filter still matches "auth".
Type "auth "
Sleep 1.4s
Screenshot "docs/img/search.png"
Backspace 5
Sleep 900ms

# 3. source filter → Codex only (green ⓒ, codex> prompt). generous gaps:
# VHS occasionally drops a rapid Ctrl+S, leaving the view on "all".
Ctrl+S
Sleep 800ms
Ctrl+S
Sleep 1.1s
Screenshot "docs/img/source-filter.png"
Ctrl+A
Sleep 600ms

# 4. hierarchical tree — drill into work/, hover web-app/ (rollup + dir preview)
Tab
Sleep 500ms
Enter
Sleep 400ms
Down
Sleep 800ms
Screenshot "docs/img/tree.png"

Sleep 300ms
