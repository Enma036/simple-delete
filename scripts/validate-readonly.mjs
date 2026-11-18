import {startupEntries,storeApps} from '../lib/windows.mjs';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
const results=await Promise.allSettled([
 startupEntries().then(rows=>({startupEntries:rows.length})),
 storeApps().then(rows=>({storePackages:rows.length,removable:rows.filter(r=>r.removable).length})),
 promisify(execFile)('powershell.exe',['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',fileURLToPath(new URL('./snapshot.ps1',import.meta.url)),'-MaxDurationMs','1000','-MaxEntries','100'],{windowsHide:true,timeout:20000,maxBuffer:8e6}).then(({stdout})=>{const data=JSON.parse(stdout);return {snapshotRegistry:data.registry.length,snapshotDirectories:data.directories.length,coverageWarnings:data.completeness.warningCount};})
]);
for(let i=0;i<results.length;i++){const r=results[i];console.log(JSON.stringify({check:['startup','store','snapshot'][i],...(r.status==='fulfilled'?{ok:true,...r.value}:{ok:false,error:r.reason.message})}));}
if(results.some(r=>r.status==='rejected'))process.exitCode=1;
