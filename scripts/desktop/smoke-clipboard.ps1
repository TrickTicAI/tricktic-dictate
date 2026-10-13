param(
    [Parameter(Mandatory=$true)][string]$Probe,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [switch]$Target,
    [string]$Case = 'normal'
)
$ErrorActionPreference = 'Stop'
if ($Target) {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.IO;
using System.Threading;
using System.Windows.Forms;
public class ClipboardTarget : Form {
    public TextBox Field = new TextBox { Multiline=true, Dock=DockStyle.Fill };
    public string Scenario;
    public ClipboardTarget(string scenario, string directory) {
        Scenario=scenario; Text="Writer synthetic clipboard acceptance"; Width=620; Height=240;
        Controls.Add(Field);
        Field.KeyDown += (sender, e) => {
            if (!e.Control || e.KeyCode != Keys.V) return;
            e.SuppressKeyPress=true;
            if (Scenario=="slow") Thread.Sleep(700);
            if (Scenario=="timeout") Thread.Sleep(3500);
            var text=Clipboard.GetText();
            Field.SelectedText=text;
            var expected=File.ReadAllText(Path.Combine(directory,"expected-"+Scenario+".txt"));
            File.WriteAllText(Path.Combine(directory,"received-"+Scenario+".json"),"{\"exact\":"+(text==expected?"true":"false")+"}");
            if (Scenario=="concurrent" && text==expected) Clipboard.SetText(text.Substring(0,text.IndexOf(" Hebrew"))+" newer copy");
        };
    }
}
'@
    $form = New-Object ClipboardTarget($Case, $OutputDirectory)
    $form.Add_Shown({
        $form.Activate(); $form.Field.Focus()
        @{window=$form.Handle.ToInt64();process=$PID} | ConvertTo-Json | Set-Content -Encoding Ascii (Join-Path $OutputDirectory "target-$Case.json")
    })
    [Windows.Forms.Application]::Run($form)
    exit
}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$receipts=@()
foreach ($scenario in @('normal','slow','timeout','concurrent','unsupported','rich','off')) {
    $targetPath=Join-Path $OutputDirectory "target-$scenario.json"
    if (Test-Path $targetPath) { throw 'Use a fresh output directory.' }
    $arguments=@('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Probe',('"'+$Probe+'"'),'-OutputDirectory',('"'+$OutputDirectory+'"'),'-Target','-Case',$scenario)
    $child=Start-Process powershell.exe -ArgumentList $arguments -PassThru
    try {
        $until=[DateTime]::UtcNow.AddSeconds(15)
        while (!(Test-Path $targetPath) -and [DateTime]::UtcNow -lt $until) { Start-Sleep -Milliseconds 100 }
        if (!(Test-Path $targetPath)) { throw "Target did not start: $scenario" }
        Start-Sleep -Milliseconds 250
        $probeProcess=Start-Process -FilePath $Probe -ArgumentList @(('"'+$targetPath+'"'),$scenario) -Wait -PassThru
        $code=$probeProcess.ExitCode
        $result=Get-Content -Raw -Encoding UTF8 (Join-Path $OutputDirectory "delivery-$scenario.json")
        $result | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory "$scenario.json")
        $result
        if ($code -ne 0) { throw "Native clipboard acceptance failed: $scenario" }
        $receipts+=($result | ConvertFrom-Json)
    } finally {
        if (!$child.HasExited) { Stop-Process -Id $child.Id }
    }
}
$receipts | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory 'results.json')
