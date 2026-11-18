import path from 'node:path';
import os from 'node:os';
import {randomUUID} from 'node:crypto';
import {lstat,readdir,realpath,mkdir,rename,readFile,writeFile} from 'node:fs/promises';
import {literal,powershell,psJson,startupEntries} from './windows.mjs';
const dataDir=path.join(process.env.LOCALAPPDATA||os.tmpdir(),'SimpleDelete');
const plans=new Map();
export function registryTarget(app){
 const key=String(app.id||'').replace(/^Microsoft\.PowerShell\.Core\\Registry::/i,'');
 if(!/^(HKEY_LOCAL_MACHINE|HKEY_CURRENT_USER)\\Software\\(?:WOW6432Node\\)?Microsoft\\Windows\\CurrentVersion\\Uninstall\\[^\\]+$/i.test(key))throw Error('Not a recognized application registration.');
 return {native:key,ps:'Registry::'+key};
}
const forbidden=new Set(['microsoft','windows','programs','commonfiles','common','software','data','apps','application','app','temp','packages','cache','system','users','programfiles','programdata']);
export function normalizeName(s){return String(s||'').toLowerCase().replace(/[^a-z0-9]/g,'');}
export function specificName(s){const n=normalizeName(s);return n.length>=4&&!forbidden.has(n);}
export function isInside(root,target){const r=path.relative(path.resolve(root),path.resolve(target));return !!r&&!r.startsWith('..')&&!path.isAbsolute(r);}
const roots=()=>[process.env.LOCALAPPDATA,process.env.APPDATA,process.env.ProgramData,process.env.ProgramFiles,process.env['ProgramFiles(x86)']].filter(Boolean).map(p=>path.resolve(p));
async function exists(p){try{await lstat(p);return true;}catch(e){if(e.code==='ENOENT')return false;throw e;}}
export async function validateTree(target,allowedRoots=roots(),limit=20000){
 target=path.resolve(target);const root=allowedRoots.find(r=>isInside(r,target));if(!root)throw Error('Target is outside an application-data root.');
 // Inspect every ancestor and every descendant: never move a junction or traverse one.
 let cursor=target;while(true){const info=await lstat(cursor);if(info.isSymbolicLink())throw Error('Junctions and symbolic links are excluded.');if(cursor===path.resolve(root))break;const parent=path.dirname(cursor);if(parent===cursor)throw Error('Invalid path boundary.');cursor=parent;}
 const realRoot=await realpath(root),realTarget=await realpath(target);if(!isInside(realRoot,realTarget))throw Error('Resolved path escaped its root.');
 let count=0;const queue=[target];while(queue.length){const p=queue.pop();if(++count>limit)throw Error('Directory exceeds the review limit; manage it manually.');const s=await lstat(p);if(s.isSymbolicLink())throw Error('Directory contains a junction or link.');if(s.isDirectory())for(const child of await readdir(p))queue.push(path.join(p,child));}
 return {count};
}
export async function scanLeftovers({name,app=null},installed=[]){
 if(!specificName(name))throw Error('Enter a specific app name of at least four letters or digits.');
 const wanted=normalizeName(name),candidates=[],warnings=[];
 for(const root of [...new Set(roots())]){
  try{for(const item of await readdir(root,{withFileTypes:true})){
   if(item.isDirectory()&&normalizeName(item.name)===wanted){const target=path.join(root,item.name);candidates.push({id:randomUUID(),kind:'directory',path:target,reason:'Exact application-name match; ownership is unproven.'});}
  }}catch(e){warnings.push(`${root}: ${e.code||e.message}`);}
 }
 if(app?.location){const target=path.resolve(app.location);if(roots().some(r=>isInside(r,target))&&specificName(path.basename(target))&&normalizeName(path.basename(target))===wanted&&await exists(target)&&!candidates.some(c=>c.path.toLowerCase()===target.toLowerCase()))candidates.push({id:randomUUID(),kind:'directory',path:target,reason:'Installer location and exact application-name match.'});}
 const registry=await psJson(`$base='HKCU:\\Software';$matches=@(Get-ChildItem -LiteralPath $base -ErrorAction SilentlyContinue | Where-Object {($_.PSChildName.ToLower() -replace '[^a-z0-9]','') -eq ${literal(wanted)}} | ForEach-Object {[pscustomobject]@{path=('HKCU:\\Software\\'+$_.PSChildName);native=('HKEY_CURRENT_USER\\Software\\'+$_.PSChildName)}});ConvertTo-Json -InputObject $matches -Compress`);
 for(const item of registry)candidates.push({id:randomUUID(),kind:'registry',path:item.path,native:item.native,reason:'Exact current-user software-key match; may contain preferences or licenses.'});
 const active=installed.some(a=>normalizeName(a.name)===wanted||a.id===app?.id);
 for(const c of candidates){c.active=active;c.blocked=(c.kind==='directory'&&path.resolve(c.path).toLowerCase()===path.resolve(dataDir).toLowerCase())||installed.some(a=>a.id!==app?.id&&a.location&&c.kind==='directory'&&(path.resolve(a.location).toLowerCase()===c.path.toLowerCase()||isInside(c.path,a.location)));}
 const plan={id:randomUUID(),name,appId:app?.id,createdAt:Date.now(),active,candidates,warnings};plans.set(plan.id,plan);
 for(const [id,p]of plans)if(Date.now()-p.createdAt>1800000)plans.delete(id);
 return {...plan,notice:'Candidates are based on exact names, not proof of ownership. Normal cleanup requires the app to be absent. Forced cleanup can break an installed app.'};
}
async function record(entry){await mkdir(path.join(dataDir,'backups'),{recursive:true});await writeFile(path.join(dataDir,'backups',entry.id+'.json'),JSON.stringify(entry,null,2));}
export async function backups(){await mkdir(path.join(dataDir,'backups'),{recursive:true});const result=[];for(const f of await readdir(path.join(dataDir,'backups'))){if(!/^[a-f0-9-]+\.json$/.test(f))continue;result.push(JSON.parse(await readFile(path.join(dataDir,'backups',f),'utf8')));}return result.sort((a,b)=>b.createdAt.localeCompare(a.createdAt));}
export async function cleanupPlan(planId,ids,{force=false,installed=[]}={}){
 const plan=plans.get(planId);if(!plan||Date.now()-plan.createdAt>1800000)throw Error('Scan expired. Scan again before cleaning.');
 if(!Array.isArray(ids)||!ids.length||ids.some(id=>!plan.candidates.some(c=>c.id===id)))throw Error('Select valid scan candidates.');
 const active=installed.some(a=>a.id===plan.appId||normalizeName(a.name)===normalizeName(plan.name));if(active&&!force)throw Error('Application is still installed. Complete uninstall and refresh, or explicitly choose forced cleanup.');
 const result=[];
 for(const c of plan.candidates.filter(c=>ids.includes(c.id))){
  try{
   if(c.blocked||installed.some(a=>a.id!==plan.appId&&a.location&&c.kind==='directory'&&(path.resolve(a.location).toLowerCase()===c.path.toLowerCase()||isInside(c.path,a.location))))throw Error('Another registered app uses this location.');
   const entry={id:randomUUID(),kind:c.kind,original:c.path,createdAt:new Date().toISOString(),status:'prepared',name:plan.name};
   if(c.kind==='directory'){
    await validateTree(c.path);entry.backup=path.join(path.dirname(c.path),'.simple-delete-quarantine',entry.id);await mkdir(path.dirname(entry.backup),{recursive:true});
    // A quarantine folder itself must not be a junction.
    if((await lstat(path.dirname(entry.backup))).isSymbolicLink())throw Error('Invalid quarantine directory.');
    await record(entry);await rename(c.path,entry.backup);
   }else{
    if(!/^HKCU:\\Software\\[^\\]+$/.test(c.path)||!specificName(c.path.split('\\').at(-1)))throw Error('Protected registry key.');
    entry.backup=path.join(dataDir,'registry',entry.id+'.reg');await mkdir(path.dirname(entry.backup),{recursive:true});await record(entry);
    await powershell(`& reg.exe export ${literal(c.native)} ${literal(entry.backup)} /y | Out-Null;if($LASTEXITCODE -ne 0){throw 'Registry backup failed'};Remove-Item -LiteralPath ${literal(c.path)} -Recurse -ErrorAction Stop`);
   }
   entry.status='quarantined';await record(entry);result.push({id:c.id,ok:true,backupId:entry.id});
  }catch(e){result.push({id:c.id,ok:false,error:e.message});}
 }
 plans.delete(planId);return result;
}
export async function restoreBackup(id){
 const entry=(await backups()).find(b=>b.id===id);if(!entry||!['quarantined','prepared'].includes(entry.status))throw Error('Backup is unavailable.');
 if(entry.kind==='directory'){
  if(await exists(entry.original))throw Error('Original location exists. Restore will not overwrite it.');
  if(path.resolve(path.dirname(entry.original))!==path.resolve(path.dirname(path.dirname(entry.backup))))throw Error('Backup is outside the original parent directory.');
  await validateTree(entry.backup);await rename(entry.backup,entry.original);
 }else if(entry.kind==='startup'){
  const key='HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run';
  await powershell(`if((Get-Item -LiteralPath ${literal(key)}).GetValueNames() -contains ${literal(entry.valueName)}){throw 'Startup value already exists'};New-ItemProperty -LiteralPath ${literal(key)} -Name ${literal(entry.valueName)} -Value ${literal(entry.command)} -PropertyType ${literal(entry.valueKind)} | Out-Null`);
 }else if(entry.kind==='registration'){
  const key=registryTarget({id:entry.original});
  await powershell(`if(Test-Path -LiteralPath ${literal(key.ps)}){throw 'Registration already exists; restore refused'};& reg.exe import ${literal(entry.backup)} | Out-Null;if($LASTEXITCODE -ne 0){throw 'Registration restore failed'}`);
 }else{
  if(!/^HKCU:\\Software\\[^\\]+$/.test(entry.original))throw Error('Invalid registry backup.');
  await powershell(`if(Test-Path -LiteralPath ${literal(entry.original)}){throw 'Original registry key exists; restore refused'};& reg.exe import ${literal(entry.backup)} | Out-Null;if($LASTEXITCODE -ne 0){throw 'Registry restore failed'}`);
 }
 entry.status='restored';entry.restoredAt=new Date().toISOString();await record(entry);return entry;
}
export async function disableStartup(id){
 const app=(await startupEntries()).find(a=>a.id===id&&a.editable);if(!app)throw Error('Only current-user Run entries can be disabled here.');
 const key='HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run';
 const kind=await powershell(`(Get-Item -LiteralPath ${literal(key)}).GetValueKind(${literal(app.name)}).ToString()`);
 if(!['String','ExpandString'].includes(kind))throw Error('Unsupported startup value type.');
 const command=await powershell(`(Get-Item -LiteralPath ${literal(key)}).GetValue(${literal(app.name)},$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)`);
 const entry={id:randomUUID(),kind:'startup',original:key,createdAt:new Date().toISOString(),name:app.name,valueName:app.name,command,valueKind:kind,status:'prepared'};await record(entry);
 await powershell(`Remove-ItemProperty -LiteralPath ${literal(key)} -Name ${literal(app.name)} -ErrorAction Stop`);entry.status='quarantined';await record(entry);return entry;
}
export async function removeRegistration(app){
 const key=registryTarget(app);
 const entry={id:randomUUID(),kind:'registration',original:key.native,createdAt:new Date().toISOString(),name:app.name,status:'prepared'};
 entry.backup=path.join(dataDir,'registry',entry.id+'.reg');await mkdir(path.dirname(entry.backup),{recursive:true});await record(entry);
 await powershell(`& reg.exe export ${literal(key.native)} ${literal(entry.backup)} /y | Out-Null;if($LASTEXITCODE -ne 0){throw 'Registration backup failed'};Remove-Item -LiteralPath ${literal(key.ps)} -Recurse -ErrorAction Stop`);
 entry.status='quarantined';await record(entry);return entry;
}
