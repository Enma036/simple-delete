import {app,BrowserWindow,dialog,Menu} from 'electron';
import {server} from '../server.mjs';
import {mkdir,appendFile,writeFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

let window,exiting=false;
const smoke=process.argv.includes('--smoke-test');
const logDir=path.join(process.env.LOCALAPPDATA||app.getPath('userData'),'SimpleDelete','logs');
async function log(error){try{await mkdir(logDir,{recursive:true});await appendFile(path.join(logDir,'desktop.log'),`${new Date().toISOString()} ${error?.stack||error}\n`);}catch{}}
process.on('uncaughtException',error=>{log(error);dialog.showErrorBox('Simple Delete encountered an error',`${error.message}\n\nDetails: ${logDir}`);});
process.on('unhandledRejection',error=>log(error));

if(!app.requestSingleInstanceLock())app.quit();
else{
 app.on('second-instance',()=>{if(window){if(window.isMinimized())window.restore();window.show();window.focus();}});
 app.whenReady().then(async()=>{
  Menu.setApplicationMenu(null);
  await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});
  const address=`http://127.0.0.1:${server.address().port}`;
  window=new BrowserWindow({width:1370,height:920,minWidth:780,minHeight:600,title:'Simple Delete',icon:fileURLToPath(new URL('../assets/app.png',import.meta.url)),backgroundColor:'#f8f9f6',show:false,autoHideMenuBar:true,webPreferences:{nodeIntegration:false,contextIsolation:true,sandbox:true}});
  window.webContents.setWindowOpenHandler(()=>({action:'deny'}));
  window.webContents.on('will-navigate',(event,url)=>{if(new URL(url).origin!==address)event.preventDefault();});
  window.webContents.session.setPermissionRequestHandler((_webContents,_permission,callback)=>callback(false));
  window.once('ready-to-show',()=>{if(!smoke)window.show();});
  window.webContents.on('did-fail-load',(_event,code,description)=>{log(`${code}: ${description}`);dialog.showErrorBox('Unable to open Simple Delete',`${description}\n\nDetails: ${logDir}`);});
  await window.loadURL(address);
  if(smoke){
   try{
    const result=await window.webContents.executeJavaScript(`(async()=>{
      const wait=async(fn)=>{const start=Date.now();while(!fn()){if(Date.now()-start>30000)throw Error('Timed out waiting for desktop UI');await new Promise(r=>setTimeout(r,100));}};
      await wait(()=>document.querySelector('#count').textContent!=='—');
      const count=Number(document.querySelector('#count').textContent);if(!count)throw Error('Inventory did not load');
      const search=document.querySelector('#search');search.value='zz-no-app-fixture-zz';search.dispatchEvent(new Event('input'));if(document.querySelector('#result-count').textContent!=='0')throw Error('Search failed');search.value='';search.dispatchEvent(new Event('input'));
      const checks={inventory:count,search:true};
      for(const tool of ['cleanup','startup','store','monitor','backups']){document.querySelector('[data-tool="'+tool+'"]').click();await wait(()=>document.querySelector('#tools-panel .tool-content')&&!document.querySelector('#tools-panel .tool-content').textContent.includes('Loading…'));if(document.querySelector('#tools-panel .error'))throw Error(tool+': '+document.querySelector('#tools-panel .error').textContent);checks[tool]=true;}
      document.querySelector('[data-view="all"]').click();if(document.querySelector('.library').hidden)throw Error('Return to library failed');checks.navigation=true;
      return checks;
    })()`);
    await writeFile(process.env.SIMPLE_DELETE_SMOKE_RESULT||path.join(logDir,'smoke.json'),JSON.stringify({ok:true,...result},null,2));
   }catch(error){await log(error);await writeFile(process.env.SIMPLE_DELETE_SMOKE_RESULT||path.join(logDir,'smoke.json'),JSON.stringify({ok:false,error:error.message},null,2));process.exitCode=1;}
   app.quit();return;
  }
  window.on('closed',()=>{window=null;app.quit();});
 }).catch(async error=>{await log(error);dialog.showErrorBox('Simple Delete could not start',`${error.message}\n\nDetails: ${logDir}`);app.quit();});
 app.on('before-quit',()=>{if(exiting)return;exiting=true;server.close();server.closeAllConnections();});
}
