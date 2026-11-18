import http from 'node:http';
import {readFile} from 'node:fs/promises';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
import {randomBytes} from 'node:crypto';
import path from 'node:path';
import {scanLeftovers,cleanupPlan,backups,restoreBackup,disableStartup,removeRegistration} from './lib/cleanup.mjs';
import {storeApps,removeStore,startupEntries,restorePoint} from './lib/windows.mjs';
import {startMonitor,stopMonitor,listMonitors,getMonitor} from './lib/monitor.mjs';
const run=promisify(execFile), root=fileURLToPath(new URL('.',import.meta.url));
const inventoryScript=(root+'scripts/inventory.ps1').replace(/app\.asar([\\/])/,'app.asar.unpacked$1');
const token=randomBytes(24).toString('hex');
let cache=null,pending=null;
const knownApps=new Map();
let mutationBusy=false;
export function parseCommand(command){
 const match=command.trim().match(/^(?:"([^"]+)"|(.+?\.exe))\s*(.*)$/i);
 if(!match)throw new Error('Unsupported uninstall command. Use Windows Settings for this app.');
 let executable=match[1]||match[2],args=match[3];
 if(/(?:^|[\\/])msiexec(?:\.exe)?$/i.test(executable)){executable='msiexec.exe';args=args.replace(/\/I(?=\s|\{)/i,'/X');}
 else if(!/^[a-z]:[\\/].+\.exe$/i.test(executable))throw new Error('Uninstaller must have an absolute executable path.');
 return {executable,args};
}
async function inventory(force=false){
 if(pending)return pending;
 if(cache&&!force)return cache;
 pending=(async()=>{const start=performance.now();const {stdout}=await run('powershell.exe',['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',inventoryScript],{windowsHide:true,maxBuffer:16e6,timeout:30000,encoding:'utf8'});const apps=JSON.parse(stdout.replace(/^\uFEFF/,''));cache={apps:Array.isArray(apps)?apps:[apps],scannedAt:new Date().toISOString(),durationMs:Math.round(performance.now()-start)};return cache;})().finally(()=>pending=null);
 return pending;
}
const json=(res,status,data)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(data));};
export const server=http.createServer(async(req,res)=>{
 res.setHeader('Cache-Control','no-store');res.setHeader('X-Content-Type-Options','nosniff');
 res.setHeader('Content-Security-Policy',"default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; img-src 'self' data:; frame-ancestors 'none'");
 if(req.headers.host!==`127.0.0.1:${server.address().port}`){json(res,403,{error:'Invalid host'});return;}
 try{
 const url=new URL(req.url,'http://127.0.0.1');
 if(req.method==='GET'&&url.pathname==='/api/session')return json(res,200,{token});
 if(req.method==='GET'&&url.pathname==='/api/apps'){const data=await inventory(url.searchParams.has('refresh'));for(const app of data.apps)knownApps.set(app.id,app);return json(res,200,{...data,apps:data.apps.map(({command,...app})=>app)});}
 if(req.method==='GET'&&url.pathname==='/api/store')return json(res,200,await storeApps());
 if(req.method==='GET'&&url.pathname==='/api/startup')return json(res,200,await startupEntries());
 if(req.method==='GET'&&url.pathname==='/api/backups')return json(res,200,await backups());
 if(req.method==='GET'&&url.pathname==='/api/monitors')return json(res,200,await listMonitors());
 if(req.method==='GET'&&url.pathname==='/api/monitor')return json(res,200,await getMonitor(url.searchParams.get('id')));
 if(req.method==='POST'&&url.pathname!=='/api/uninstall'){
  if(req.headers['x-session-token']!==token)return json(res,403,{error:'Invalid session'});
  if(mutationBusy)return json(res,409,{error:'Another operation is still running. Please wait.'});
  mutationBusy=true;
  try{
  let raw='';for await(const chunk of req){raw+=chunk;if(raw.length>64000)return json(res,413,{error:'Request too large'});}
  const body=JSON.parse(raw||'{}');
  if(url.pathname==='/api/scan'){const app=knownApps.get(body.id);return json(res,200,await scanLeftovers({name:app?.name||body.name,app},(await inventory(true)).apps));}
  if(url.pathname==='/api/cleanup'){if(body.confirm!=='CLEAN')throw Error('Explicit cleanup confirmation required.');return json(res,200,await cleanupPlan(body.planId,body.ids,{force:body.force===true,installed:(await inventory(true)).apps}));}
  if(url.pathname==='/api/restore')return json(res,200,await restoreBackup(body.id));
  if(url.pathname==='/api/registration/remove'){
   if(body.confirm!=='REMOVE REGISTRATION')throw Error('Explicit registration removal confirmation required.');
   const app=(await inventory(true)).apps.find(a=>a.id===body.id);if(!app)throw Error('Registration no longer exists. Refresh your library.');
   const result=await removeRegistration(app);cache=null;return json(res,200,result);
  }
  if(url.pathname==='/api/startup/disable')return json(res,200,await disableStartup(body.id));
  if(url.pathname==='/api/store/remove'){if(body.confirm!=='REMOVE')throw Error('Removal confirmation required.');await removeStore(body.id);return json(res,200,{message:'Package removed for the current user.'});}
  if(url.pathname==='/api/restore-point'){await restorePoint();return json(res,200,{message:'Restore point request completed. Check Windows System Protection for availability.'});}
  if(url.pathname==='/api/monitor/start')return json(res,200,await startMonitor({label:String(body.label||'Installation session').slice(0,120),targetHint:String(body.label||'').slice(0,120)}));
  if(url.pathname==='/api/monitor/stop')return json(res,200,await stopMonitor(body.id));
  return json(res,404,{error:'Unknown action'});
  }finally{mutationBusy=false;}
 }
 if(req.method==='POST'&&url.pathname==='/api/uninstall'){
 if(req.headers['x-session-token']!==token)return json(res,403,{error:'Invalid session'});
 let body='';for await(const chunk of req){body+=chunk;if(body.length>10000)return json(res,413,{error:'Request too large'});}
 const {id}=JSON.parse(body);const app=(await inventory()).apps.find(a=>a.id===id);if(!app?.command)return json(res,400,{error:'No registered uninstaller'});
 const {executable,args}=parseCommand(app.command);
 const literal=s=>"'"+s.replaceAll("'","''")+"'";
 const script=`$ErrorActionPreference='Stop'; Start-Process -FilePath ${literal(executable)} ${args?'-ArgumentList '+literal(args):''}`;
 await run('powershell.exe',['-NoProfile','-NonInteractive','-EncodedCommand',Buffer.from(script,'utf16le').toString('base64')],{windowsHide:true,timeout:15000});
 return json(res,200,{message:'Uninstaller launched. Complete its prompts, then refresh your library.'});
 }
 if(req.method==='GET'&&url.pathname==='/icon.svg'){res.setHeader('Content-Type','image/svg+xml');res.end(await readFile(root+'assets/icon.svg'));return;}
 const files={'/':'index.html','/app.js':'app.js','/tools.js':'tools.js','/style.css':'style.css','/tools.css':'tools.css'};
 if(req.method!=='GET'||!files[url.pathname])return json(res,404,{error:'Not found'});
 res.setHeader('Content-Type',url.pathname.endsWith('.js')?'text/javascript':url.pathname.endsWith('.css')?'text/css':'text/html');res.end(await readFile(root+'public/'+files[url.pathname]));
 }catch(error){json(res,500,{error:error.message});}
});
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))server.listen(Number(process.env.PORT||4781),'127.0.0.1',()=>console.log(`Simple Delete: http://127.0.0.1:${server.address().port}`));
