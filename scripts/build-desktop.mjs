import {spawn} from 'node:child_process';
import {createRequire} from 'node:module';
const require=createRequire(import.meta.url);
const child=spawn(process.execPath,[require.resolve('electron-builder/cli.js'),'--win','portable','--x64'],{stdio:'inherit',env:{...process.env,ELECTRON_BUILDER_COMPRESSION_LEVEL:'3'}});
child.on('error',error=>{console.error(error.message);process.exitCode=1;});
child.on('exit',code=>process.exitCode=code||0);
