import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,symlink,rm} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {specificName,isInside,validateTree,registryTarget} from '../lib/cleanup.mjs';
import {literal} from '../lib/windows.mjs';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
test('broad system and data names cannot select cleanup candidates',()=>{for(const name of ['Microsoft','Windows','Program Files','Common Files','Packages','Apps','Temp','a'])assert.equal(specificName(name),false);assert.equal(specificName('Discord'),true);});
test('containment rejects root, parent and sibling-prefix targets',()=>{const root=path.resolve('cleanup-test');assert.equal(isInside(root,root),false);assert.equal(isInside(root,path.resolve('cleanup-test-sibling')),false);assert.equal(isInside(root,path.join(root,'..')),false);assert.equal(isInside(root,path.join(root,'app')),true);});
test('tree checks refuse targets outside roots, links and oversized traversals',async t=>{
 const root=await mkdtemp(path.join(os.tmpdir(),'simple-delete-test-'));t.after(()=>rm(root,{recursive:true,force:true}));const app=path.join(root,'App');await mkdir(app);await writeFile(path.join(app,'config'),'test');
 assert.equal((await validateTree(app,[root])).count,2);await assert.rejects(validateTree(root,[root]),/outside/);await assert.rejects(validateTree(app,[root],1),/limit/);
 const junction=path.join(root,'linked');await symlink(app,junction,'junction');await assert.rejects(validateTree(junction,[root]),/Junctions/);
 await symlink(app,path.join(app,'nested-link'),'junction');await assert.rejects(validateTree(app,[root]),/link/);
});
test('registration removal is constrained to direct uninstall entries',()=>{
 const good='Microsoft.PowerShell.Core\\Registry::HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Example';assert.equal(registryTarget({id:good}).native,good.split('::')[1]);
 for(const id of ['HKEY_LOCAL_MACHINE\\Software','HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall','HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\App\\Subkey'])assert.throws(()=>registryTarget({id}));
});
test('PowerShell literals keep apostrophes and metacharacters inside data',()=>assert.equal(literal("App's; $(test)"),"'App''s; $(test)'"));
test('reviewed fixture cleanup quarantines and restores without overwriting',async t=>{
 const root=await mkdtemp(path.join(os.tmpdir(),'simple-delete-recovery-'));t.after(()=>rm(root,{recursive:true,force:true}));
 const name='SimpleDeleteFixture'+Date.now(),target=path.join(root,name);await mkdir(target);await writeFile(path.join(target,'settings.txt'),'preserve me');
 const code=`import assert from 'node:assert/strict';import {access,readFile,mkdir,rm} from 'node:fs/promises';import path from 'node:path';import {scanLeftovers,cleanupPlan,restoreBackup} from './lib/cleanup.mjs';const name=process.env.FIXTURE_NAME,target=path.join(process.env.LOCALAPPDATA,name);const plan=await scanLeftovers({name},[]);assert.equal(plan.candidates.length,1);const [result]=await cleanupPlan(plan.id,[plan.candidates[0].id]);assert.equal(result.ok,true);await assert.rejects(access(target));await mkdir(target);await assert.rejects(restoreBackup(result.backupId),/exists/);await rm(target,{recursive:true});await restoreBackup(result.backupId);assert.equal(await readFile(path.join(target,'settings.txt'),'utf8'),'preserve me');await assert.rejects(cleanupPlan(plan.id,[plan.candidates[0].id]),/expired/);`;
 await promisify(execFile)(process.execPath,['--input-type=module','-e',code],{cwd:path.resolve('.'),env:{...process.env,LOCALAPPDATA:root,APPDATA:root,ProgramData:root,ProgramFiles:root,'ProgramFiles(x86)':root,FIXTURE_NAME:name},timeout:20000});
});
