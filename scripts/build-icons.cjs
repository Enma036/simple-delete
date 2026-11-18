// Render the vector master: node_modules/electron/dist/electron.exe scripts/build-icons.cjs
const {app, BrowserWindow} = require('electron');
const fs = require('node:fs/promises');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
app.disableHardwareAcceleration();
app.whenReady().then(async () => {
  const window = new BrowserWindow({show:false, width:256, height:256, transparent:true,
    webPreferences:{offscreen:true, backgroundThrottling:false}});
  try {
    const svg = await fs.readFile(path.join(root, 'assets/icon.svg'), 'utf8');
    await window.loadURL('data:text/html;charset=utf-8,' + encodeURIComponent(
      `<style>html,body{margin:0;background:transparent;overflow:hidden}svg{display:block;width:100vw;height:100vh}</style>${svg}`));
    const images = [];
    for (const size of [16,24,32,48,64,128,256]) {
      window.setContentSize(size,size);
      await window.webContents.executeJavaScript('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
      const capture = await window.webContents.capturePage();
      images.push({size, png:capture.resize({width:size,height:size}).toPNG()});
    }
    await fs.writeFile(path.join(root, 'assets/app.png'), images.at(-1).png);
    const header = Buffer.alloc(6 + images.length * 16);
    header.writeUInt16LE(1,2);
    header.writeUInt16LE(images.length,4);
    let offset = header.length;
    images.forEach(({size,png},index) => {
      const entry = 6 + index * 16;
      header[entry] = header[entry+1] = size === 256 ? 0 : size;
      header.writeUInt16LE(1,entry+4);
      header.writeUInt16LE(32,entry+6);
      header.writeUInt32LE(png.length,entry+8);
      header.writeUInt32LE(offset,entry+12);
      offset += png.length;
    });
    await fs.writeFile(path.join(root, 'assets/app.ico'), Buffer.concat([header,...images.map(i => i.png)]));
    console.log('Built app.png and app.ico from assets/icon.svg');
  } finally { window.destroy(); app.quit(); }
}).catch(error => {console.error(error); app.exit(1);});
