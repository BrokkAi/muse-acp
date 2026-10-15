import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import release

# Loads only the function definitions from install.ps1, so nothing is
# downloaded, and shadows the registry read with the MUSE_ACP_TEST_REGISTRY
# value (unset means the registry read fails).
HARNESS = r'''
param([string]$Installer)
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Installer, [ref]$null, [ref]$errors)
if ($errors.Count -gt 0) { throw ($errors | Out-String) }
$functions = $ast.FindAll({ $args[0] -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)
foreach ($function in $functions) {
    . ([scriptblock]::Create($function.Extent.Text))
}
function Get-ItemProperty([string]$LiteralPath, [string]$Name) {
    if ($null -eq $env:MUSE_ACP_TEST_REGISTRY) { throw 'registry unavailable' }
    if ($LiteralPath -ne 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' -or
        $Name -ne 'PROCESSOR_ARCHITECTURE') {
        throw "no registry value $LiteralPath\$Name"
    }
    [pscustomobject]@{ PROCESSOR_ARCHITECTURE = $env:MUSE_ACP_TEST_REGISTRY }
}
try {
    Resolve-Target
}
catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
'''

POWERSHELL = shutil.which('pwsh') or shutil.which('powershell')


@unittest.skipUnless(POWERSHELL, 'PowerShell is not installed')
class InstallerArchitecture(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.harness = Path(temp.name) / 'harness.ps1'
        self.harness.write_text(HARNESS)

    def resolve(self, registry=None, wow64=None, process=None):
        env = {k: v for k, v in os.environ.items()
               if k.upper() not in ('PROCESSOR_ARCHITECTURE', 'PROCESSOR_ARCHITEW6432', 'MUSE_ACP_TEST_REGISTRY')}
        for name, value in (('MUSE_ACP_TEST_REGISTRY', registry), ('PROCESSOR_ARCHITEW6432', wow64),
                            ('PROCESSOR_ARCHITECTURE', process)):
            if value is not None:
                env[name] = value
        return subprocess.run([POWERSHELL, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
                               '-File', str(self.harness), str(Path('install.ps1').resolve())],
                              env=env, capture_output=True, text=True)

    def assertTarget(self, expected, **architecture):
        result = self.resolve(**architecture)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), expected)

    def test_native_x64(self):
        self.assertTarget('x86_64-pc-windows-msvc', registry='AMD64', process='AMD64')

    def test_native_arm64(self):
        self.assertTarget('aarch64-pc-windows-msvc', registry='ARM64', process='ARM64')

    def test_emulated_x64_on_arm64_uses_the_registry(self):
        # x64 PowerShell under emulation sees AMD64 and no W6432 hint.
        self.assertTarget('aarch64-pc-windows-msvc', registry='ARM64', process='AMD64')

    def test_wow64_hint_when_the_registry_is_unavailable(self):
        self.assertTarget('aarch64-pc-windows-msvc', wow64='ARM64', process='x86')

    def test_process_architecture_when_nothing_else_is_available(self):
        self.assertTarget('x86_64-pc-windows-msvc', process='amd64')

    def test_unsupported_architecture_fails(self):
        result = self.resolve(registry='x86', process='x86')
        self.assertEqual(result.returncode, 1)
        self.assertIn('unsupported architecture: x86', result.stderr)


class TargetLists(unittest.TestCase):
    """The release targets are listed in several languages; they must agree."""

    def test_launcher_matches_release(self):
        launcher = Path('npm/bin/muse-acp.cjs').read_text()
        targets = re.findall(r"'(?:darwin|linux|win32)-(?:x64|arm64)': '([^']+)'", launcher)
        self.assertCountEqual(targets, release.TARGETS)

    def test_release_workflow_matches_release(self):
        workflow = Path('.github/workflows/release.yml').read_text()
        self.assertCountEqual(re.findall(r'^\s+target: (\S+)$', workflow, re.M), release.TARGETS)

    def test_windows_installer_matches_release(self):
        installer = Path('install.ps1').read_text()
        windows = [target for target in release.TARGETS if 'windows' in target]
        self.assertCountEqual(re.findall(r"'([a-z0-9_]+-pc-windows-msvc)'", installer), windows)


if __name__ == '__main__':
    unittest.main()
