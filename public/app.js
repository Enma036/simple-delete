import {closeTools,scanApp} from './tools.js';
let apps=[],view='all',page=0,session='',selected=null,history=[];
const $=s=>document.querySelector(s),pageSize=12;
const escape=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const size=n=>n>0?n>=1024?(n/1024).toFixed(1)+' GB':Math.round(n)+' MB':'Not reported';
function date(s){if(!/^\d{8}$/.test(s||''))return 'Not reported';return `${s.slice(0,4)}-${s.slice(4,6)}-${s.slice(6,8)}`;}
function notify(message){$('#status').textContent=message;$('#status').hidden=false;}
async function request(url,options){const res=await fetch(url,options);const data=await res.json();if(!res.ok)throw Error(data.error||'Request failed');return data;}
function render(){
 const query=$('#search').value.toLowerCase().trim(),filter=$('#filter').value;
 let rows=apps.filter(a=>(a.name+' '+(a.publisher||'')).toLowerCase().includes(query)&&(view!=='large'||a.sizeMB>=1024)&&(view!=='recent'||/^\d{8}$/.test(a.installed||''))&&(filter!=='removable'||a.canUninstall)&&(filter!=='large'||a.sizeMB>=1024));
 const sort=$('#sort').value;
 rows.sort((a,b)=>sort==='size'?(b.sizeMB||0)-(a.sizeMB||0):sort==='date'?String(b.installed||'').localeCompare(String(a.installed||'')):a.name.localeCompare(b.name));
 const pages=Math.max(1,Math.ceil(rows.length/pageSize));page=Math.min(page,pages-1);
 $('#result-count').textContent=rows.length;
 $('#empty').hidden=true;
 if(view==='history'){
 $('#apps').innerHTML='';$('.table-scroll').hidden=true;$('#empty').hidden=false;$('#empty').textContent=history.length?history.join(' • '):'No uninstallers launched in this session.';
 $('#showing').textContent='Activity records launches, not completed removals.';$('#prev').disabled=true;$('#next').disabled=true;return;
 }
 $('.table-scroll').hidden=false;
 $('#apps').innerHTML=rows.slice(page*pageSize,(page+1)*pageSize).map(a=>{
 const index=apps.indexOf(a),hue=[...a.name].reduce((n,c)=>n+c.charCodeAt(0),0)%360;
 return `<tr><td><div class="app-cell"><span class="app-icon" style="--tint:hsl(${hue} 40% 94%);--ink:hsl(${hue} 35% 38%)">${escape(a.name[0].toUpperCase())}</span><div><div class="app-name">${escape(a.name)}</div><div class="app-version">${escape(a.version||'Version not reported')}</div></div></div></td><td>${escape(a.publisher||'Not reported')}</td><td>${size(a.sizeMB)}</td><td>${date(a.installed)}</td><td><button class="uninstall" data-scan="${index}">Scan</button> <button class="uninstall" data-index="${index}" ${a.canUninstall?'':'disabled'}>Uninstall ↗</button></td></tr>`;
 }).join('');
 if(!rows.length){$('#empty').hidden=false;$('#empty').textContent='No applications match. Try a different search or filter.';}
 $('#showing').textContent=rows.length?`Showing ${page*pageSize+1}–${Math.min((page+1)*pageSize,rows.length)} of ${rows.length} applications`:'0 applications';
 $('#page').textContent=`${page+1} / ${pages}`;$('#prev').disabled=page===0;$('#next').disabled=page===pages-1;
}
async function load(refresh=false){
 $('#refresh').disabled=true;$('#scan-time').textContent='Reading Windows registry…';
 try{const data=await request('/api/apps'+(refresh?'?refresh=1':''));apps=data.apps;$('#count').textContent=apps.length;$('#nav-count').textContent=apps.length;$('#size').textContent=size(apps.reduce((n,a)=>n+(a.sizeMB||0),0));$('#large').textContent=apps.filter(a=>a.sizeMB>=1024).length;$('#scan-time').textContent=`Registry read in ${(data.durationMs/1000).toFixed(2)}s · ${new Date(data.scannedAt).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'})}`;render();}
 catch(e){notify(e.message);$('#scan-time').textContent='Could not read library';$('#empty').hidden=false;$('#empty').textContent='Inventory unavailable. Refresh to try again.';$('#showing').textContent='Unable to load applications';}
 finally{$('#refresh').disabled=false;}
}
$('#refresh').onclick=()=>load(true);
for(const id of ['search','filter','sort'])$('#'+id).addEventListener(id==='search'?'input':'change',()=>{page=0;render();});
$('#prev').onclick=()=>{page--;render();};$('#next').onclick=()=>{page++;render();};
document.querySelectorAll('[data-view]').forEach(button=>button.onclick=()=>{closeTools();view=button.dataset.view;page=0;document.querySelector('.nav.active')?.classList.remove('active');button.classList.add('active');const names={all:'All applications',large:'Large applications',recent:'Recently installed',history:'Session activity'};$('#crumb').textContent=names[view];$('#list-title').firstChild.textContent=(view==='all'?'Application library':names[view])+' ';if(view==='recent')$('#sort').value='date';render();});
$('#apps').onclick=e=>{const button=e.target.closest('[data-index]');if(!button)return;selected=apps[Number(button.dataset.index)];$('#dialog-title').textContent=`Uninstall ${selected.name}?`;$('#dialog-copy').textContent=`${selected.publisher||'Publisher not reported'} · ${size(selected.sizeMB)}`;$('#confirm').showModal();};
$('#confirm-uninstall').onclick=async()=>{const button=$('#confirm-uninstall');button.disabled=true;try{if(!session)session=(await request('/api/session')).token;const data=await request('/api/uninstall',{method:'POST',headers:{'Content-Type':'application/json','X-Session-Token':session},body:JSON.stringify({id:selected.id})});history.unshift(`${new Date().toLocaleTimeString()} — ${selected.name}: uninstaller launched`);$('#confirm').close();notify(data.message);if(view==='history')render();}catch(e){notify(e.message);}finally{button.disabled=false;}};
document.addEventListener('keydown',e=>{if(e.key==='/'&&!['INPUT','SELECT','TEXTAREA'].includes(document.activeElement.tagName)&&!$('#confirm').open){e.preventDefault();$('#search').focus();}if(e.key==='Escape')$('#status').hidden=true;});
load();
