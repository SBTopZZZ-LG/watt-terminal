# Watt shell integration for PowerShell (pwsh 7 and Windows PowerShell 5.1). Mirrors integration.zsh: the same events,
# field names and escaping, so the Rust scanner (core/scanner.rs) reads both the same way.
# Loaded by the -Command of the launch (core/session.rs, Windows only), after the user's $PROFILE. The file's text goes
# through ScriptBlock.Create, so an execution policy does not block it. Needs WATT_NONCE in the environment; it is removed
# from the environment after reading.
# Every OSC 133/633 event carries n=<nonce>. Event order per prompt: E, C (command) / D (exit), OSC 7 and Cwd (cwd),
# then A, the prompt text and B. The OSC terminator is ST (ESC \) only.

if ($global:WattIntegrated) { [Console]::Write("watt: already integrated`r`n"); return }
$global:WattIntegrated = $true

[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$global:WattNonce = [string]$env:WATT_NONCE
Remove-Item Env:WATT_NONCE -ErrorAction SilentlyContinue
$global:WattEsc = [string][char]27
$global:WattBs = [string][char]92
$global:WattTerm = $global:WattEsc + $global:WattBs
$global:WattRan = $false
$global:WattNoOp = $false
# True while the wrapper prompt runs. A replacement prompt that calls the wrapper (a chain) re-enters it; see prompt().
$global:WattInPrompt = $false

# Dev only (WATT_HISTFILE, set by the app or the tests): PSReadLine saves history to that file instead of the user's.
# Unset, which is the normal case, leaves the history alone. Removed from the environment after reading, like the nonce.
if ($env:WATT_HISTFILE) { Set-PSReadLineOption -HistorySavePath $env:WATT_HISTFILE }
Remove-Item Env:WATT_HISTFILE -ErrorAction SilentlyContinue

# OSC emitter: ESC ] body terminator. Written straight to the console so it bypasses the pipeline.
function global:_WattOsc([string]$body) {
  [Console]::Out.Write($global:WattEsc + ']' + $body + $global:WattTerm)
  [Console]::Out.Flush()
}

# E / 633;Cwd payload escaping: backslash -> \\, ';' -> \x3b, C0 and DEL -> \xHH. Same as _watt_escape_e.
function global:_WattEscapeE([string]$s) {
  $sb = [System.Text.StringBuilder]::new()
  foreach ($ch in $s.ToCharArray()) {
    $n = [int]$ch
    if ($n -eq 92) { [void]$sb.Append($global:WattBs + $global:WattBs) }
    elseif ($n -eq 59) { [void]$sb.Append('\x3b') }
    elseif ($n -lt 32 -or $n -eq 127) { [void]$sb.Append(('\x{0:x2}' -f $n)) }
    else { [void]$sb.Append($ch) }
  }
  $sb.ToString()
}

# OSC 7 path: Windows drive paths become /C:/..., bytes outside [a-zA-Z0-9/._~:-] are percent-encoded (UTF-8).
function global:_WattFileUrlPath([string]$p) {
  $p = $p -replace '\\', '/'
  if ($p -match '^[A-Za-z]:') { $p = '/' + $p }
  $sb = [System.Text.StringBuilder]::new()
  foreach ($b in [System.Text.Encoding]::UTF8.GetBytes($p)) {
    $c = [char]$b
    if ($b -lt 128 -and ($c -match '[a-zA-Z0-9/._~:-]')) { [void]$sb.Append($c) }
    else { [void]$sb.Append(('%{0:X2}' -f $b)) }
  }
  $sb.ToString()
}

# Hello values lose ';' and control bytes, like _watt_hello.
function global:_WattText([string]$v) { ($v -replace '[\x00-\x1f;]', '') }

# True when PSReadLine would keep the buffer open for more input (unterminated string/block/pipe). This approximates
# PSReadLine's ValidateAndAcceptLine decision with the parser's error ids (probed in pwsh 7.6.9 and 5.1).
function global:_WattIncomplete([string]$s) {
  $errs = $null
  [void][System.Management.Automation.Language.Parser]::ParseInput($s, [ref]$null, [ref]$errs)
  $incompleteIds = @('MissingEndCurlyBrace', 'MissingEndParenthesisInExpression', 'MissingEndSquareBracket',
    'TerminatorExpectedAtEndOfString', 'EmptyPipeElement', 'MissingCommandAfterPipe', 'ExpectedValueExpression',
    'IncompleteString')
  foreach ($e in $errs) { if ($incompleteIds -contains $e.ErrorId) { return $true } }
  $false
}

# True when a complete line runs nothing: blank or comment only, or only function definitions. Such a line has no
# command status of its own, so the prompt reports exit 0 instead of the stale $?. A line with a parse error is not one.
function global:_WattNoOp([string]$s) {
  $errs = $null
  $ast = [System.Management.Automation.Language.Parser]::ParseInput($s, [ref]$null, [ref]$errs)
  if ($errs -and $errs.Count) { return $false }
  $block = $ast.EndBlock
  if ($block) {
    foreach ($st in $block.Statements) {
      if ($st -isnot [System.Management.Automation.Language.FunctionDefinitionAst]) { return $false }
    }
  }
  $true
}

# Hello: the observer treats a shell that has said Hello as observable. Printed last, after the user's profile.
$__ver = _WattText ([string]$PSVersionTable.PSVersion)
$__user = _WattText ([string]$env:USERNAME)
$__host = _WattText ([string]$env:COMPUTERNAME)
_WattOsc ("633;Hello;n=$($global:WattNonce);v=1;sh=pwsh;ver=$__ver;user=$__user;host=$__host;pid=$PID")

# Wrap the prompt that exists now (the user's, or PowerShell's default).
$global:WattOrigPrompt = (Get-Item -Path Function:\prompt -ErrorAction SilentlyContinue).ScriptBlock
if (-not $global:WattOrigPrompt) { $global:WattOrigPrompt = { 'PS> ' } }

# The cwd pair of the prompt: OSC 7 (the local cwd) then 633;Cwd. Shared by prompt() and _WattIdle.
function global:_WattCwdOsc {
  if ($PWD.Provider.Name -eq 'FileSystem') {
    $p = $PWD.ProviderPath
    _WattOsc ("7;file://" + [string]$env:COMPUTERNAME + (_WattFileUrlPath $p))
    _WattOsc ("633;Cwd;n=$($global:WattNonce);" + (_WattEscapeE $p))
  }
}

function global:prompt {
  # First statements capture the previous command's status before anything else runs.
  $ok = $?
  $lec = $global:LASTEXITCODE
  # Marker: _WattCheckPrompt finds copies of this block by its text ('watt-prompt-wrapper'). Keep it in the body.
  $null = 'watt-prompt-wrapper'
  # A replacement that calls this prompt from inside its own text (a chain) re-enters here. The outer call does all the
  # work, so the inner one writes nothing and returns '' for the chain to compose with.
  if ($global:WattInPrompt) { return '' }
  $global:WattInPrompt = $true
  try {
    $nonce = $global:WattNonce
    # PSReadLine can lose the Enter binding after integration (see _WattBindEnter).
    $null = _WattBindEnter
    if ($global:WattRan) {
      $global:WattRan = $false
      $code = 0
      if (-not $ok -and -not $global:WattNoOp) { if ($lec) { $code = [int]$lec } else { $code = 1 } }
      $global:WattNoOp = $false
      _WattOsc ("133;D;n=$nonce;exit=$code;ps=$code")
    }
    _WattCwdOsc
    $text = [string](& $global:WattOrigPrompt)
  } finally {
    $global:WattInPrompt = $false
  }
  return ($global:WattEsc + ']133;A;n=' + $nonce + $global:WattTerm) + $text + ($global:WattEsc + ']133;B;n=' + $nonce + $global:WattTerm)
}

# The prompt function as it was installed above. A Function:\prompt that is not this block was defined by someone else.
$global:WattPromptBlock = (Get-Item -Path Function:\prompt).ScriptBlock

# A prompt redefined after integration (profile, conda, posh-git, oh-my-posh lazy init) has no A/B/D/cwd. Called from the
# Enter handler, before the line runs: the new prompt becomes the one to wrap, the wrapper goes back, and Degraded says so.
# A copy of the wrapper (its text has the marker) is not adopted, since the wrapper would then call itself. The wrapper just goes back.
# A previous command that never got its D gets one now, exit 0 as a guess, so the observer does not see C over an open record.
function global:_WattCheckPrompt {
  $cur = (Get-Item -Path Function:\prompt -ErrorAction SilentlyContinue).ScriptBlock
  if ([object]::ReferenceEquals($cur, $global:WattPromptBlock)) { return }
  if ($global:WattRan) {
    $global:WattRan = $false
    $global:WattNoOp = $false
    _WattOsc ("133;D;n=$($global:WattNonce);exit=0;ps=0")
  }
  $isCopy = ($null -ne $cur) -and $cur.ToString().Contains('watt-prompt-wrapper')
  if (-not $isCopy) {
    $orig = $cur
    if ($null -eq $orig) { $orig = { 'PS> ' } }
    $global:WattOrigPrompt = $orig
  }
  Set-Item -Path Function:\global:prompt -Value $global:WattPromptBlock
  _WattOsc ("633;Degraded;n=$($global:WattNonce);reason=prompt")
}

# Runs on PowerShell.OnIdle: about every 300 ms while PSReadLine waits at the prompt, not while a command runs. A line that
# redefines the prompt and then does work (function prompt { ... }; echo x) leaves no prompt() call to repair it, so the
# record stays open and the observer stays Running until the next Enter. Here the repair happens without input. The
# guessed D exits 0 even if the line failed. A prompt that is still the wrapper returns at once, so an idle shell prints nothing.
# Output is OSC only: writing text here would corrupt the prompt line. Errors are swallowed, since an action must not print.
function global:_WattIdle {
  try {
    $cur = (Get-Item -Path Function:\prompt -ErrorAction SilentlyContinue).ScriptBlock
    if ([object]::ReferenceEquals($cur, $global:WattPromptBlock)) { return }
    $wasRan = $global:WattRan
    _WattCheckPrompt
    if ($wasRan) {
      # The replaced prompt text is already on screen; A and B go right after it, with the cwd first, as prompt() does.
      _WattCwdOsc
      _WattOsc ('133;A;n=' + $global:WattNonce)
      _WattOsc ('133;B;n=' + $global:WattNonce)
    }
  } catch {}
}

# Binds Enter to $global:WattEnter. In Vi mode PSReadLine keeps separate Insert and Command maps, so both get it.
# prompt() calls this each time, because a runtime Set-PSReadLineOption -EditMode Vi or Set-PSReadLineKeyHandler -Key Enter
# drops the binding. Measured at about 0.4 ms per call (pwsh 7.6.9, PSReadLine 2.4.5).
function global:_WattBindEnter {
  Set-PSReadLineKeyHandler -Key Enter -ScriptBlock $global:WattEnter
  if ([string](Get-PSReadLineOption).EditMode -eq 'Vi') {
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock $global:WattEnter -ViMode Insert
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock $global:WattEnter -ViMode Command
  }
}

# Enter: emit E and C for a complete, non-blank line, then hand off to PSReadLine's normal validate-and-accept.
# Incomplete input (open block/string/pipe) gets no E/C but a 133;P (continuation, PS2) like zsh; the next Enter that
# completes the line emits E and C with the whole text. ValidateAndAcceptLine inserts a newline as usual.
$global:WattEnter = {
  _WattCheckPrompt
  $line = $null
  $cursor = $null
  [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$cursor)
  if (-not [string]::IsNullOrWhiteSpace($line)) {
    if (_WattIncomplete $line) {
      _WattOsc ('133;P;n=' + $global:WattNonce + ';k=s')
    } else {
      $global:WattRan = $true
      $global:WattNoOp = _WattNoOp $line
      $global:LASTEXITCODE = 0
      # The output starts on the row below the cursor at C (observer.rs), so the cursor must be on the last input row.
      [Microsoft.PowerShell.PSConsoleReadLine]::SetCursorPosition($line.Length)
      _WattOsc ('633;E;n=' + $global:WattNonce + ';' + (_WattEscapeE $line))
      _WattOsc ('133;C;n=' + $global:WattNonce)
    }
  }
  [Microsoft.PowerShell.PSConsoleReadLine]::ValidateAndAcceptLine()
}
$null = _WattBindEnter

# Idle repair (_WattIdle). Registered once: $global:WattIdleSub guards a second run of this script in the same shell.
if (-not $global:WattIdleSub) {
  $global:WattIdleSub = $true
  $null = Register-EngineEvent -SourceIdentifier PowerShell.OnIdle -Action { _WattIdle }
}
