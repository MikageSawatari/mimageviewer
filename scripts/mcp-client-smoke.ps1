# Ask Codex CLI (or Claude Code CLI) to call the mIV MCP tools and print what was called.
# The MCP server is registered only for this run; client settings are not changed.
# Start the disposable portable mIV first (target\portable-smoke\mimageviewer.exe).
# Usage:
#   .\scripts\mcp-client-smoke.ps1
#   .\scripts\mcp-client-smoke.ps1 -Client claude
#   .\scripts\mcp-client-smoke.ps1 -PromptFile .\target\prompt.txt
param(
    [ValidateSet('codex', 'claude')][string] $Client = 'codex',
    [string] $PromptFile = ''
)

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$argList = @('-I', (Join-Path $root 'scripts\mcp_client_smoke.py'), '--client', $Client)
if ($PromptFile -ne '') { $argList += @('--prompt-file', $PromptFile) }
python @argList
exit $LASTEXITCODE
