# Troubleshooting: what to check, and what to tell the user

Read `state` first. It shows what Watt is using right now.

## A tool is missing, or the error says “unavailable”
The error means no tool with that name is offered to you right now. It can be a mistyped name, a tool that never existed, or a tool from an MCP server that is off or not connected. Check `state`, under “MCP servers”, for the server that owns the tool, then tell the user the matching step:
- off (`disabled`): turn it on with the “MCP servers” button and its switch, or in Settings, “MCP”, with the “Enabled” checkbox.
- `not started yet` or `connecting…`: it starts with the app or the first turn. Wait a few seconds and try again. The “Starting MCP servers…” loader near the message box shows progress.
- `failed`: quote the failure message from `state`. Tell the user to open Settings, “MCP”, then “Logs” on that server, and then “Restart”.
- `needs_auth`: the server needs a sign-in. The message says how. After it, “Restart”.
- Not listed in `state` at all: the server was removed, or the tool has another name. Do not guess names.
- The tool is set to “Never (hide tools)”: the user changes that server's approval, or that tool's, in Settings, “MCP”.
If the tool is a built-in one that is missing, the shell may have exited. Then the user clicks “Start new session” in the message box.

## An approval was denied, or a call was refused
- Denied by the user: with “Stop” the turn ends; with “Keep working” you get an error for that call and carry on. Do not retry the same command.
- A call shown as “refused” was never asked. Check “Mode” and the deny rules in Settings, “Approvals”. A message that says the tool is disabled means it is turned off (see the MCP steps above).
- A “Remote shells” or nested shell: its commands may ask even when approve-all is on, because approve-all applies to the shell it was switched on in.

## A skill does not show, or does not load
- Not in `state` under `Skills in use`: its folder is not scanned, or its SKILL.md is not valid. Settings, “Skills”, “Warnings” says why.
- `switched off`: the user turns it on with its “Use” checkbox.
- `hidden from the model`: the skill is set to be listed only when asked. The user can see it, but you cannot load it.
- `shadowed`: a skill of the same name earlier in the folder order is used instead. The name `watt` is reserved for this built-in skill, so a folder skill with that name is ignored.
- Approval “Ask first”: the user approves a card before the skill loads, once per turn. “Don't ask” loads it with no card.

## settings.json is broken
`state` says `settings.json: broken`, or that Watt runs on the last working copy. The file is not used as it is saved, and the MCP servers and skill folders may not be what the user expects. Tell the user to open Settings (“AI settings”): it shows the reason and the recovery actions. Do not try to repair settings.json yourself, and do not ask the user to paste its contents.

## A reply was redone
The chat shows “Command finished: redoing the reply.” when a command you were using finished while you were replying. This is normal: the reply is written again with the command's result. It is not an error. An idle agent is not woken when a command exits. At most three redos happen in one turn.

## Writing a file on a remote shell
write_file and edit_file reach only this machine. On a remote shell, write a multi-line file in one run_command with a quoted heredoc: `cat > file <<'EOF'`, the lines, then `EOF`. Do not chain printf or echo with nested quotes; one missing quote stops the whole line.

## What to ask the user
- The exact label or message they see, and which “AI settings” tab they are on.
- Whether the approval chip shows “manual” or “bypass”.
- Never ask for an API key, token, password or header value. Say it is typed into the matching field in Settings, and stored there, not in the chat.
