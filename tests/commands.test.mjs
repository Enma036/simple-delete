import {test} from 'node:test';
import assert from 'node:assert/strict';
import {parseCommand} from '../server.mjs';
test('quoted paths keep arguments intact',()=>assert.deepEqual(parseCommand('"C:\\Program Files\\App\\uninstall.exe" /remove'),{executable:'C:\\Program Files\\App\\uninstall.exe',args:'/remove'}));
test('MSI maintenance commands become removal commands',()=>assert.deepEqual(parseCommand('MsiExec.exe /I{123-ABC}'),{executable:'msiexec.exe',args:'/X{123-ABC}'}));
test('unquoted executable paths are supported',()=>assert.equal(parseCommand('C:\\Apps\\uninstall.exe /S').args,'/S'));
test('shell commands and relative executables are refused',()=>{for(const command of ['cmd /c del x','uninstall.exe /remove','powershell.exe -Command x','https://example.com'])assert.throws(()=>parseCommand(command));});
