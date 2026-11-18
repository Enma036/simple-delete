import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
export const literal=value=>"'"+String(value).replaceAll("'","''")+"'";
export async function powershell(script,timeout=30000){
 const {stdout}=await promisify(execFile)('powershell.exe',['-NoProfile','-NonInteractive','-EncodedCommand',Buffer.from("$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);"+script,'utf16le').toString('base64')],{windowsHide:true,encoding:'utf8',maxBuffer:16e6,timeout});
 return stdout.trim().replace(/^\uFEFF/,'');
}
export async function psJson(script){const out=await powershell(script);return out?JSON.parse(out):[];}
export async function storeApps(){return psJson("$apps=@(Get-AppxPackage | Select-Object @{n='id';e={$_.PackageFullName}},@{n='name';e={$_.Name}},Publisher,Version,@{n='removable';e={-not $_.NonRemovable -and -not $_.IsFramework -and -not $_.IsResourcePackage}});ConvertTo-Json -InputObject $apps -Compress");}
export async function removeStore(id){const apps=await storeApps();if(!apps.find(a=>a.id===id&&a.removable))throw Error('Package cannot be removed for this user.');await powershell(`Remove-AppxPackage -Package ${literal(id)}`,120000);}
export async function restorePoint(){await powershell("$before=@(Get-ComputerRestorePoint | Select-Object -ExpandProperty SequenceNumber);Checkpoint-Computer -Description 'Simple Delete — before cleanup' -RestorePointType MODIFY_SETTINGS;$created=@(Get-ComputerRestorePoint | Where-Object {$before -notcontains $_.SequenceNumber});if($created.Count -eq 0){throw 'Windows did not create a new restore point. Check elevation, System Protection and restore-point frequency limits.'}",120000);}
export async function startupEntries(){return psJson("$items=@(foreach($key in @('HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run','HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run')){if(Test-Path -LiteralPath $key){$p=Get-ItemProperty -LiteralPath $key;foreach($name in $p.PSObject.Properties.Name | Where-Object {$_ -notlike 'PS*'}){[pscustomobject]@{id=($key+'|'+$name);name=$name;command=$p.$name;scope= $(if($key.StartsWith('HKCU')){'Current user'}else{'Machine'});editable=$key.StartsWith('HKCU')}}}});ConvertTo-Json -InputObject $items -Compress");}
