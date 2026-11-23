# Watt UI: where things are, and how to do each task

Text in curly quotes is exact UI text. Keyboard shortcuts are for Windows and Linux; on macOS use Cmd for Ctrl.

## Where things are
- Settings: the “AI settings” dialog. Open it with Ctrl+, (Cmd+, on macOS), the gear button at the end of the status strip (it is labelled “AI settings”), or `/model` with no argument. Its tabs are “Models”, “Approvals”, “MCP”, “Skills”, “Limits”, “Terminal” and “About”.
- Status strip: shows the provider and the model. It has the “Integrate this shell” button when the shell in front is not integrated yet.
- Message box: “Message the agent”. Files are attached with the “Attach files” button.
- Approval chip, above the message box: shows “manual” normally, or “bypass” when approve-all is on for this session. Shift+Tab flips it. It is never saved, so it ends with the session.
- MCP servers button: “MCP servers”, next to the attach button. It opens a panel with one switch per server.
- Agent panel: its header has the “New conversation” icon button, the “Conversation history” button, and the “Agent panel position” group (Dock left and Dock right; the panel docks left or right only). “Show the agent panel” brings it back when it is hidden.
- Find in chat: “Find in chat” (Ctrl+F while focus is in the agent panel). Find in the terminal: “Find in terminal” (Ctrl+F in the terminal, or Ctrl+Shift+F at any time).
- Todo list: the “Todo list” panel, from todo_write.
- Context meter: opens the “Context breakdown” panel.
- Slash commands typed in the message box: /help lists them with the custom commands. Built in: /new, /stop, /model, /export, /context, /history, /rename, /btw, /compact, /fork, /plan, /review, /init.

## Turn an MCP server on or off
1. Quickest: click “MCP servers” next to the attach button. Each server row has a switch labelled with the server's name. Its tooltip says “The agent can use this server” when on, and “Hidden from the agent” when off. The change takes effect on the agent's next step.
2. Or in Settings: “AI settings”, then the “MCP” tab, then the “Enabled” checkbox on that server's row.
A row shows “off”, “connecting…”, “1 tool” or “1 tool”, “failed”, or “needs sign-in”. A failed or sign-in row shows its message on hover. A server that is off is not offered to the agent, so its tools are missing for it.

## Add an MCP server
Settings, then “MCP”, then “Add server”. Pick a preset in “Preset” (for example “Custom” or “You.com (free, no key)”), or use the “Form” tab or the “Paste JSON” tab. Fill it in, then click “Add server”. Secret values are typed into the form's value fields, never into the chat.

## Edit, restart, read the logs, or remove a server
Settings, then “MCP”. Each row has “Edit”, “Restart”, “Logs” and “Remove”. Changes are saved with “Save server”. “Logs” shows the last lines the server printed, redacted.

## Add a provider and its models
Settings, then “Models”, then “Providers”, then “Add provider”. In the provider editor: “Headers” (“Add header”), then “Models” (“Fetch models” to read the list from the provider, or “Add custom model”), then “Save provider”. “Delete provider” removes it. Its API key is typed in the editor and stored in the secret store (the keychain, or a file where no keychain is available). It is never shown again.

## Change the default model
Settings, then “Models”, then “Default model”: choose the “Model”, and then a “Variant” if the model has one. “None (mock agent)” turns the agent off. The choice is used for new messages. `/model provider/model` switches it from the message box.

## Change the context limit
Settings, then “Models”, then “Global context limit”: type the “Tokens” value. Empty means no limit. “Clear” removes it.

## Approvals
Settings, then “Approvals”:
- “Mode”: “Manual” asks for every command. “Regex rules” lets the “Allow rules” and “Deny rules” decide. A deny rule wins over an allow rule.
- “When I deny a call”: “Keep working” sends the agent an error for that call and it carries on. “Stop” ends the turn.
- “Remote shells”: “Treat SSH and nested shells like local” is off by default, so allow rules ask in those shells.
- “Let the agent integrate shells without asking” is off by default.
- Save with “Save approvals”.
Approve-all is the approval chip above (manual or bypass). Cards ask with “Approve” and “Deny” buttons.

## Skills
Settings, then “Skills”:
- “Skill folders”: “Add folder” opens the sheet “Add skill folder”, where “Browse…” opens a folder picker. Each row has “Edit” and “Remove”; “Reset to defaults” restores the default folders. The “watt config” folder is locked.
- “Loading a skill”: “Ask first” shows a card before a skill loads the first time in a turn. “Don't ask” loads it with no card. Each skill row has its own select (“Global default”, “Ask first”, “Don't ask”), and its “Use” checkbox switches it off.
- The built-in “watt” skill is listed with the tag “Built in”. It has no switch and no select: “Always available”.
- “Warnings” at the bottom lists skills that were skipped, and why.

## Limits
Settings, then “Limits”: “Model calls per turn”, “Polling calls per turn”, “Commands per turn” and “Wait time per turn (minutes)”. Each is checked as it is typed.

## Terminal tab
Settings, then “Terminal”:
- “Hide command separators and agent labels in the terminal” (a checkbox).
- “Queued messages”: “Send them together, as one message (default)” or “Send them one at a time, one per turn”.
- “Reasoning”, under “How reasoning blocks show in the agent”: “Collapsed”, “Auto” or “Expanded”. A reasoning block shows as “Thinking”, then “Thought for” the seconds. Reasoning is not stored: it is gone after a reload or after switching conversation.
- “Suggested prompts”: “Suggest follow-up prompts after each reply”.
- “Notifications”: notify when the agent is done or needs the user while Watt is in the background.
- “Developer”: “Enable the layout inspector”.

## Conversations
- New conversation: the “New conversation” icon in the agent panel header. The old one stays in “Conversation history”.
- `/fork` copies the conversation to a new one and switches to it. `/compact` summarizes it into a new one; the old one stays in history. `/btw <question>` asks a side question that is not stored.

## Terminal and chat menus
- Right-click in the terminal: “Copy”, “Paste”, “Select all”, “Find in terminal”, and “Clear terminal” at the prompt when the agent is idle.
- Right-click in the chat: “Copy”, “Copy message”, “Select all” and “Find in chat”.

## Shortcuts
- Ctrl+, (Cmd+,): open “AI settings”.
- Ctrl+F: find in the focus zone (the chat when focus is in the agent panel, else the terminal). Ctrl+Shift+F: find in the terminal.
- Ctrl+= , Ctrl+- and Ctrl+0: zoom in, out and reset.
- Shift+Tab in the message box: flip approve-all.

## Attachments and images
Image thumbnails, in the message box and in sent messages, open a full-size view. Close it with the “Close image” button, Esc, or a click outside the image. If the file is gone the view says “Full image unavailable.” Text-file chips do not open anything.

## Where the config lives
The `Config folder` and `Data folder` lines of `state` give the paths. Settings are in settings.json in the config folder. You cannot read that file: ask the user to open Settings to see or change it.
