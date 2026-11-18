const $=s=>document.querySelector(s);
const esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
let token='',currentTool='',plan=null;
async function api(route,body){if(body&&!token)token=(await (await fetch('/api/session')).json()).token;const res=await fetch('/api/'+route,body?{method:'POST',headers:{'Content-Type':'application/json','X-Session-Token':token},body:JSON.stringify(body)}:{});const data=await res.json();if(!res.ok)throw Error(data.error||'Request failed');return data;}
function message(s){$('#status').textContent=s;$('#status').hidden=false;}
async function confirmRegistration(){
 const modal=document.createElement('dialog');
 modal.innerHTML='<form method="dialog"><h2>Remove stale registration?</h2><p>This removes the installed-app entry. Application files remain. A registry backup will be saved.</p><label class="tool-form">Type REMOVE REGISTRATION<input aria-label="Registration removal confirmation" autocomplete="off"></label><div class="dialog-actions"><button class="button" value="cancel">Cancel</button><button class="button primary" value="remove">Remove registration</button></div></form>';
 document.body.append(modal);const input=modal.querySelector('input');
 modal.querySelector('form').onsubmit=e=>{if(e.submitter?.value==='remove'&&input.value!=='REMOVE REGISTRATION'){e.preventDefault();input.setCustomValidity('Type REMOVE REGISTRATION exactly.');input.reportValidity();}};
 input.oninput=()=>input.setCustomValidity('');
 return new Promise(resolve=>{modal.onclose=()=>{resolve(modal.returnValue==='remove'&&input.value==='REMOVE REGISTRATION');modal.remove();};modal.showModal();input.focus();});
}
const nav=document.createElement('div');nav.className='tools-nav';nav.innerHTML='<p class="nav-label">TOOLS</p>'+[['cleanup','⌕','Leftover cleanup'],['monitor','◉','Install monitoring'],['startup','↗','Startup manager'],['store','▧','Windows apps'],['backups','↶','Backups & recovery']].map(([id,icon,label])=>`<button class="nav" data-tool="${id}"><span>${icon}</span>${label}</button>`).join('');$('nav').after(nav);
const panel=document.createElement('section');panel.id='tools-panel';panel.className='library tools-panel';panel.hidden=true;$('.library').after(panel);
const labels={cleanup:'Leftover cleanup',monitor:'Installation monitoring',startup:'Startup manager',store:'Windows applications',backups:'Backups & recovery'};
nav.onclick=e=>{const button=e.target.closest('[data-tool]');if(button)openTools(button.dataset.tool);};
export function closeTools(){panel.hidden=true;$('.library').hidden=false;nav.querySelector('.active')?.classList.remove('active');currentTool='';}
export async function openTools(tool){
 currentTool=tool;$('.nav.active')?.classList.remove('active');nav.querySelector(`[data-tool="${tool}"]`)?.classList.add('active');$('.library').hidden=true;panel.hidden=false;$('#crumb').textContent=labels[tool];panel.innerHTML=`<div class="tool-heading"><h2>${labels[tool]}</h2></div><div class="tool-content"><p class="muted">Loading…</p></div>`;
 try{await renderTool(tool);}catch(e){panel.querySelector('.tool-content').innerHTML=`<p class="error">${esc(e.message)}</p><button class="button" data-action="reload">Try again</button>`;}
}
function content(html){panel.querySelector('.tool-content').innerHTML=html;}
function item(title,detail,action){return `<article class="tool-item"><div><strong>${esc(title)}</strong><p>${esc(detail)}</p></div>${action||''}</article>`;}
async function renderTool(tool){
 if(tool==='cleanup'){
  content('<p>Review app folders and current-user registry keys before removing them. Exact-name matches are candidates; they may include personal settings and licenses.</p><form id="scan-form" class="tool-form"><label>Application name<input id="cleanup-name" required minlength="4" placeholder="Exact application name, e.g. Discord"></label><button class="button primary">Scan leftovers</button></form><p class="muted">Normal cleanup is available after uninstall. Forced cleanup also permits candidates for installed apps. Protected roots, junctions, and locations used by other registered apps are excluded.</p><div id="scan-results"></div>');
  $('#scan-form').onsubmit=async e=>{e.preventDefault();await busy(e.submitter,()=>scan({name:$('#cleanup-name').value}));};
 }else if(tool==='monitor'){
  const monitors=await api('monitors');if(currentTool!==tool)return;
  content('<p>Take a baseline, run an installer yourself, then finish the session to compare changes. Snapshots capture directory metadata and uninstall entries; concurrent changes cannot be proven to belong to the installer.</p><form id="monitor-form" class="tool-form"><label>Session name<input id="monitor-label" required maxlength="120" placeholder="Application being installed"></label><button class="button primary">Start baseline</button></form><div>'+monitors.map(m=>item(m.label||'Installation session',`${m.status} · ${new Date(m.startedAt).toLocaleString()} · ${m.completeness.warningCount} coverage warnings`,m.status==='active'?`<button class="button" data-stop="${esc(m.id)}">Finish & compare</button>`:`<button class="button" data-report="${esc(m.id)}">View changes</button>`)).join('')+'</div><div id="monitor-report"></div>');
  $('#monitor-form').onsubmit=async e=>{e.preventDefault();await busy(e.submitter,async()=>{await api('monitor/start',{label:$('#monitor-label').value});message('Baseline saved. Run your installer, then finish this session.');await openTools('monitor');});};
 }else if(tool==='startup'){
  const rows=await api('startup');if(currentTool!==tool)return;
  content('<p>Manage registry Run entries. Current-user entries can be disabled with a backup. Machine entries are read-only here. Scheduled tasks, services and Startup-folder shortcuts are outside this view.</p>'+rows.map(r=>item(r.name,`${r.scope} · ${r.command}`,`<button class="button" data-disable="${esc(r.id)}" ${r.editable?'':'disabled'}>Disable</button>`)).join('')+(!rows.length?'<p>No registry startup entries found.</p>':''));
 }else if(tool==='store'){
  const rows=await api('store');if(currentTool!==tool)return;
  content('<p>Installed Store / MSIX packages for the current user. Frameworks, resource packages, and non-removable packages are protected. Removal is handled by Windows and can take a moment.</p>'+rows.map(r=>item(r.name,`${r.Version} · ${r.Publisher}`,`<button class="button" data-package="${esc(r.id)}" ${r.removable?'':'disabled'}>${r.removable?'Remove':'Protected'}</button>`)).join(''));
 }else if(tool==='backups'){
  const rows=await api('backups');if(currentTool!==tool)return;
  content('<p>Selected app folders are moved into a quarantine folder on the same volume. Registry keys are exported before removal. Restores never overwrite an existing destination.</p><button class="button" data-action="restore-point">Create Windows restore point</button><p class="muted">Restore points require an elevated server and enabled System Protection. Windows may limit how often a point can be created. Errors are shown here; the app will not silently elevate.</p>'+rows.map(r=>item(r.name,`${r.kind} · ${r.status} · ${r.original}`,`<button class="button" data-restore="${esc(r.id)}" ${['prepared','quarantined'].includes(r.status)?'':'disabled'}>Restore</button>`)).join('')+(!rows.length?'<p class="muted">No backups yet.</p>':''));
 }
}
async function busy(button,fn){if(button)button.disabled=true;try{await fn();}catch(e){message(e.message);}finally{if(button)button.disabled=false;}}
async function scan(body){plan=await api('scan',body);$('#scan-results').innerHTML=`<h3>${esc(plan.name)} — ${plan.candidates.length} candidates</h3><p class="muted">${esc(plan.notice)}</p>${plan.warnings.map(w=>`<p class="error">${esc(w)}</p>`).join('')}${plan.candidates.map(c=>`<label class="candidate"><input type="checkbox" value="${esc(c.id)}" ${c.blocked?'disabled':''}><span><strong>${esc(c.path)}</strong><small>${esc(c.reason)} ${c.blocked?'Blocked: shared application location.':''}</small></span><b>${esc(c.kind)}</b></label>`).join('')}${plan.candidates.length?'<label class="force"><input id="force-cleanup" type="checkbox"> Forced cleanup: I understand this may break an installed app.</label><button class="button primary" data-action="clean">Quarantine selected items</button>':'<p>No exact-name candidates found in the supported roots.</p>'}`;}
export async function scanApp(app){await openTools('cleanup');$('#cleanup-name').value=app.name;await busy(null,async()=>{await scan({id:app.id});const section=document.createElement('div');section.className='registered-tools';section.innerHTML=`<h3>Broken uninstaller?</h3><p>Removing a registration hides this app from the installed-app list. It does not uninstall files, services, or drivers. Use this only for a stale or broken entry after reviewing leftover candidates. A registry backup is saved first.</p><button class="button" data-registration="${esc(app.id)}">Remove stale registration</button>`;$('#scan-results').append(section);});}
panel.onclick=async e=>{
 const b=e.target.closest('button');if(!b||b.form)return;
 await busy(b,async()=>{
  if(b.dataset.action==='reload')return openTools(currentTool);
  if(b.dataset.action==='restore-point'){if(!confirm('Create a Windows restore point now?'))return;const r=await api('restore-point',{});message(r.message);}
  if(b.dataset.action==='clean'){
   const ids=[...panel.querySelectorAll('.candidate input:checked')].map(i=>i.value),force=$('#force-cleanup').checked;
   if(!ids.length)throw Error('Select the candidates you have reviewed.');
   if(!confirm(`Quarantine ${ids.length} selected items for ${plan.name}? ${force?'Forced cleanup may break this app. ':''}Registry selections will be backed up and removed. Folders will be moved.`))return;
   const results=await api('cleanup',{planId:plan.id,ids,force,confirm:'CLEAN'});$('#scan-results').innerHTML='<h3>Cleanup results</h3>'+results.map(r=>item(plan.candidates.find(c=>c.id===r.id)?.path||r.id,r.ok?'Quarantined. Restore from Backups & recovery.':r.error)).join('');
  }
  if(b.dataset.stop){if(!confirm('Capture the final snapshot and compare this installation session?'))return;await api('monitor/stop',{id:b.dataset.stop});await openTools('monitor');}
  if(b.dataset.report){const r=await api('monitor?id='+encodeURIComponent(b.dataset.report));const c=r.comparison;$('#monitor-report').innerHTML=`<h3>Observed changes</h3><p>${esc(c.attributionAnalysis.statement)}</p><p>${c.completeness.complete?'Complete within configured scope.':'Partial coverage — inspect warnings below.'}</p><pre class="report">${esc(JSON.stringify({summary:c.observedChanges.summary,warnings:c.completeness.warnings,changes:c.observedChanges,associations:c.attributionAnalysis.directoryAssociations},null,2))}</pre>`;}
  if(b.dataset.disable){if(!confirm('Disable this current-user startup entry? A backup will be saved for restoration.'))return;await api('startup/disable',{id:b.dataset.disable});message('Startup entry disabled. Restore it from Backups & recovery.');await openTools('startup');}
  if(b.dataset.package){if(!confirm('Remove this package for your current Windows user? Windows handles removal; app data may be removed.'))return;await api('store/remove',{id:b.dataset.package,confirm:'REMOVE'});message('Package removed for the current user.');await openTools('store');}
  if(b.dataset.restore){if(!confirm('Restore this backup? Existing destinations will not be overwritten.'))return;await api('restore',{id:b.dataset.restore});message('Backup restored.');await openTools('backups');}
  if(b.dataset.registration){if(!await confirmRegistration())return;await api('registration/remove',{id:b.dataset.registration,confirm:'REMOVE REGISTRATION'});b.disabled=true;message('Registration backed up and removed. Refresh the library to update it.');}
 });
};
