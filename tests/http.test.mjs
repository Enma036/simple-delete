import {test} from 'node:test';
import assert from 'node:assert/strict';
import {server} from '../server.mjs';
import http from 'node:http';
test('loopback server serves tools and rejects unauthorized mutation requests',async()=>{
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
 try{
  const url=`http://127.0.0.1:${server.address().port}`;
  for(const file of ['/','/tools.js','/tools.css']){const res=await fetch(url+file);assert.equal(res.status,200);assert.ok(res.headers.get('content-security-policy'));await res.text();}
  for(const route of ['/api/cleanup','/api/store/remove','/api/startup/disable','/api/registration/remove','/api/restore-point','/api/restore','/api/monitor/start','/api/uninstall']){const res=await fetch(url+route,{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});assert.equal(res.status,403);await res.text();}
  const badHost=await new Promise((resolve,reject)=>{http.get(url,{headers:{Host:'attacker.example'}},res=>{res.resume();resolve(res.statusCode);}).on('error',reject);});assert.equal(badHost,403);
  const res=await fetch(url+'/api/session');assert.match((await res.json()).token,/^[a-f0-9]{48}$/);
 }finally{await new Promise(resolve=>server.close(resolve));}
});
