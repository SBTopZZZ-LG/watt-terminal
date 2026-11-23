---
name: watt
description: Cheatsheet for Watt, the terminal app. Use it when the user asks how to do something in Watt, or a tool is missing or unavailable. Covers screens, settings, MCP servers, skills, approvals and the live setup (file state).
---
# Watt, for the agent

Load this when the user asks how to do something in Watt, or when a tool you expected is missing or unavailable.

Files of this skill (read one with the skill tool and its `file` argument):
- `ui.md`: where things are, and the steps for each common task. Labels are exact UI text.
- `troubleshooting.md`: what to check when a tool is missing, an approval is refused, a skill does not show, or settings are broken.
- `state`: a live, redacted snapshot of this setup, read now. It has the app version, the folders, each provider (chat host only, API key set or not set, header names only), each MCP server with its live state, the approvals, the limits and the skills. Read it first when the answer depends on this setup.

Rules:
- You cannot read settings.json or the secret store, and Watt's own folders are refused to the file tools. Use `state` instead. It never contains secret values, URL paths or queries, header values, command arguments or env values.
- Never ask the user to paste an API key, token, password or header value into the chat. Tell them to type it into the matching field in Settings.
- Quote UI text exactly as the file gives it. If you are not sure of a label, say so and describe where the control is.
- You cannot see the user's screen through this skill. Say what to click; do not claim what the screen shows.
- This skill runs nothing and grants nothing. Approvals still apply to every command.

Answer with the steps first, then what to check if they do not work. Keep it short.
