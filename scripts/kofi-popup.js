#!/usr/bin/env node
/**
 * Ko-fi Stream Alert popup listener.
 *
 * Connects to Ko-fi's Azure SignalR alert stream (same one the overlay page
 * uses) and, on every donation, pops up a Firefox window with the animated
 * Ko-fi alert (+ jingle / TTS sound) and fires a native desktop notification.
 *
 * Zero dependencies: Node >= 22 (native fetch + WebSocket).
 *
 * Usage:
 *   node scripts/kofi-popup.js            # run listener (foreground)
 *   node scripts/kofi-popup.js --test     # fire a fake alert to verify the popup
 *   node scripts/kofi-popup.js --setup    # create firefox profile + cache assets, then exit
 *
 * Installed as a systemd user service: systemctl --user start kofi-popup
 */
'use strict';

const fs = require('fs');
const path = require('path');
const os = require('os');
const { execFile, spawn } = require('child_process');

// ---------------------------------------------------------------------------
// Ko-fi account / overlay identity (from the user's stream-alert overlay URL)
// ---------------------------------------------------------------------------
const CFG = {
  pageId: 'S2P125Z0D8',
  userKey: 'sa_bc6daca8-624a-4b7d-be12-435f5b1bbef1',
  signalrUserId:
    'HgBUxpcNnjK1B-s-1xiSRJvZx8OQnKix-pl-4jO3mF9aGllfnolXGXO5-s-xFu5d3lISy-s-q',
  overlayUrl:
    'https://ko-fi.com/streamalerts/overlay/' + 'sa_bc6daca8-624a-4b7d-be12-435f5b1bbef1',
  hubUrl: 'https://kofi.service.signalr.net/client/?hub=streamalerts',
  profileName: 'kofi-popup',
  dataDir: path.join(os.homedir(), '.local', 'share', 'kofi-popup'),
  logFile: path.join(os.homedir(), '.local', 'state', 'kofi-popup.log'),
  popupMs: 13000,      // animation is 12s (6 slide + 5 stay + 1 fade) + buffer
  notifyCmd: '/usr/bin/notify-send',
  firefoxCmd: '/usr/bin/firefox',
};

const BROWSER_HEADERS = {
  'User-Agent':
    'Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0',
  Accept: '*/*',
  'Accept-Language': 'en-US,en;q=0.9',
  'x-requested-with': 'XMLHttpRequest',
  Referer: CFG.overlayUrl,
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------
function log(msg) {
  const line = `[${new Date().toISOString()}] ${msg}`;
  console.log(line);
  try {
    fs.mkdirSync(path.dirname(CFG.logFile), { recursive: true });
    fs.appendFileSync(CFG.logFile, line + '\n');
  } catch {}
}

function run(cmd, args, opts = {}) {
  return new Promise((resolve) => {
    execFile(cmd, args, { timeout: 30000, ...opts }, (err, stdout, stderr) => {
      resolve({ ok: !err, stdout: (stdout || '').trim(), stderr: (stderr || '').trim(), err });
    });
  });
}

function spawnBg(cmd, args) {
  const child = spawn(cmd, args, { detached: true, stdio: 'ignore' });
  child.unref();
}

function notify(title, body) {
  run(CFG.notifyCmd, ['-u', 'normal', '-t', String(CFG.popupMs), '-a', 'Ko-fi', title, body || '']);
}

async function fetchBrowser(url, opts = {}) {
  return fetch(url, {
    ...opts,
    headers: { ...BROWSER_HEADERS, ...(opts.headers || {}) },
  });
}

// ---------------------------------------------------------------------------
// Asset cache: ko-fi CSS + jingle sound (mirrors what the overlay page loads)
// ---------------------------------------------------------------------------
// NOTE: /api/streamalerts/active-style and /sounds/jingle.wav are served with
// browser-session auth (406/404 to plain clients) — the popup page fetches
// them in real Firefox instead (see buildPopupHtml).
const ASSETS = {
  'css.css': 'https://ko-fi.com/Content/css.css?v=922ca',
  'stream-alerts.css': 'https://ko-fi.com/Content/stream-alerts.css?v=922ca',
};

async function refreshAssets() {
  fs.mkdirSync(CFG.dataDir, { recursive: true });
  for (const [file, url] of Object.entries(ASSETS)) {
    try {
      const res = await fetchBrowser(url);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const buf = Buffer.from(await res.arrayBuffer());
      // guard against Cloudflare challenge pages sneaking into the cache
      if (buf.length > 0 && buf.slice(0, 15).toString().includes('<!DOCTYPE html')) {
        throw new Error('Cloudflare challenge page');
      }
      fs.writeFileSync(path.join(CFG.dataDir, file), buf);
      log(`cached ${file} (${buf.length} bytes)`);
    } catch (e) {
      log(`WARN could not cache ${file}: ${e.message}`);
    }
  }
}

// ---------------------------------------------------------------------------
// Ko-fi SignalR negotiation (mirrors the overlay page's JS)
// ---------------------------------------------------------------------------
async function getConnectionInfo() {
  const tokenRes = await fetchBrowser(
    `https://ko-fi.com/api/streamalerts/negotiation-token?userKey=${CFG.userKey}`
  );
  if (!tokenRes.ok) throw new Error(`negotiation-token HTTP ${tokenRes.status}`);
  const { token } = await tokenRes.json();

  const url = `https://sa-functions.ko-fi.com/api/negotiate?negotiationToken=${encodeURIComponent(
    token
  )}&pageId=${CFG.pageId}&timestamp=${Date.now()}`;
  const res = await fetchBrowser(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
      'x-ms-signalr-userid': CFG.signalrUserId,
    },
  });
  if (!res.ok) throw new Error(`negotiate HTTP ${res.status}`);
  const info = await res.json();
  info.accessToken = info.accessToken || info.accessKey;
  info.url = info.url || info.endpoint;
  return info;
}

// ---------------------------------------------------------------------------
// SignalR listener
// ---------------------------------------------------------------------------
let socket = null;
let retryDelay = 5000;
let closeTimer = null;

function connectSignalR() {
  getConnectionInfo()
    .then((info) => {
      log('negotiated SignalR connection');
      const sep = info.url.includes('?') ? '&' : '?';
      const wsUrl = `${info.url}${sep}access_token=${encodeURIComponent(info.accessToken)}`;
      socket = new WebSocket(wsUrl);

      socket.addEventListener('open', () => {
        retryDelay = 5000;
        log('SignalR connected — listening for donations');
        notify('Ko-fi alerts armed', 'Waiting for your next donation ☕');
        // SignalR JSON protocol handshake (record separator terminated)
        socket.send('{"protocol":"json","version":1}\x1e');
      });

      socket.addEventListener('message', (ev) => {
        const data = typeof ev.data === 'string' ? ev.data : ev.data.toString();
        for (const frame of data.split('\x1e')) {
          if (!frame.trim()) continue;
          handleFrame(frame);
        }
      });

      socket.addEventListener('close', () => onDisconnect('closed'));
      socket.addEventListener('error', () => onDisconnect('error'));
    })
    .catch((e) => {
      log(`WARN negotiation failed: ${e.message}`);
      onDisconnect('negotiation failed');
    });
}

function onDisconnect(reason) {
  if (socket) {
    try { socket.close(); } catch {}
    socket = null;
  }
  log(`disconnected (${reason}); reconnecting in ${retryDelay / 1000}s`);
  setTimeout(connectSignalR, retryDelay);
  retryDelay = Math.min(retryDelay * 2, 60000);
}

function handleFrame(frame) {
  let msg;
  try { msg = JSON.parse(frame); } catch { return; }
  if (msg.type === 6) return; // ping / handshake ack
  if (msg.type === 1 && msg.target === 'newStreamAlert') {
    const html = typeof msg.arguments?.[0] === 'string' ? msg.arguments[0] : '';
    const tts = typeof msg.arguments?.[1] === 'string' ? msg.arguments[1] : null;
    log(`newStreamAlert received${html ? ` (${html.length} bytes html)` : ''}${tts ? ` — tts: "${tts.slice(0, 80)}"` : ''}`);
    if (html) enqueueAlert(html, tts);
  } else if (msg.type === 1) {
    log(`hub invocation: ${msg.target}`);
  } else if (msg.type === 7) {
    log(`server error: ${JSON.stringify(msg.error || msg)}`);
  }
}

// ---------------------------------------------------------------------------
// Alert popup pipeline (serialized)
// ---------------------------------------------------------------------------
let queue = Promise.resolve();
let currentPopupTs = 0;

function enqueueAlert(html, tts) {
  queue = queue.then(() => showAlert(html, tts)).catch((e) => log(`WARN popup failed: ${e.message}`));
}

function parseDonation(html) {
  const text = html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
  // try to find a like in: <div class="x-from-name">Bakar</div>
  let name = null;
  const nameMatch = html.match(/class="[^"]*(?:from|name)[^"]*"[^>]*>\s*([^<]{1,40}?)\s*</i);
  if (nameMatch && !/^(?:left|right|top|bottom)/i.test(nameMatch[1])) name = nameMatch[1];
  let amount = null;
  const amtMatch = text.match(/([\$€£]\s?\d+(?:[.,]\d{2})?|\d+(?:[.,]\d{2})?\s?(?:USD|EUR|GBP|CAD|AUD))/i);
  if (amtMatch) amount = amtMatch[1].replace(/\s+/g, ' ');
  return { name: name || 'someone', amount: amount || 'a donation', text };
}

async function fetchTtsWav(ttsMessage) {
  if (!ttsMessage) return null;
  const form = new URLSearchParams({
    ttsMessage,
    voice: 'en-GB-AbbiNeural',
    userKey: CFG.userKey,
  });
  const res = await fetchBrowser('https://ko-fi.com/api/streamalerts/generate-tts-audio', {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: form.toString(),
  });
  if (!res.ok) return null;
  const data = await res.json();
  if (!data.success || !data.ttsUrl) return null;
  const wavRes = await fetchBrowser(data.ttsUrl);
  if (!wavRes.ok) return null;
  return Buffer.from(await wavRes.arrayBuffer());
}

async function showAlert(html, ttsMessage) {
  const ts = Date.now();
  currentPopupTs = ts;
  log('showing popup alert');

  // fetch TTS audio (optional; degrade gracefully)
  let ttsBuf = null;
  try { ttsBuf = await fetchTtsWav(ttsMessage); } catch { ttsBuf = null; }
  if (ttsBuf) fs.writeFileSync(path.join(CFG.dataDir, 'tts.wav'), ttsBuf);

  // popup page — stable file for OBS Browser Source + timestamped for Firefox
  const htmlContent = buildPopupHtml(html, ts);
  const file = path.join(CFG.dataDir, `alert-${ts}.html`);
  const stableFile = path.join(CFG.dataDir, 'alert.html');
  fs.writeFileSync(file, htmlContent);
  fs.writeFileSync(stableFile, htmlContent);
  const fileUrl = `file://${file}`;

  spawnBg(CFG.firefoxCmd, [
    '-P', CFG.profileName, '-no-remote', '--new-window', fileUrl,
  ]);

  // position bottom-left, always-on-top
  positionWindow('kofi-alert', 1000, 560);

  // close after animation
  setTimeout(() => {
    if (currentPopupTs === ts) {
      run('pkill', ['-f', CFG.profileName]).catch(() => {});
      try { fs.unlinkSync(file); } catch {}
      log('popup closed');
    }
  }, CFG.popupMs);
}

async function positionWindow(titlePattern, w, h) {
  for (let i = 0; i < 20; i++) {
    const found = await run('xdotool', ['search', '--name', titlePattern]);
    if (found.ok && found.stdout) {
      const wid = found.stdout.split('\n')[0];
      const geo = await run('xdotool', ['getdisplaygeometry']);
      const [sw, sh] = geo.ok ? geo.stdout.split(/\s+/).map(Number) : [1920, 1080];
      const x = Math.max(0, Math.floor(sw * 0.03));
      const y = Math.max(0, sh - h - Math.floor(sh * 0.03));
      await run('xdotool', ['windowsize', wid, String(w), String(h)]);
      await run('xdotool', ['windowmove', wid, String(x), String(y)]);
      await run('xdotool', ['windowstate', '--add', 'ABOVE', wid]);
      return;
    }
    await new Promise((r) => setTimeout(r, 400));
  }
  log('WARN could not locate popup window for positioning');
}

function buildPopupHtml(alertHtml, ts) {
  const d = CFG.dataDir;
  const ku = encodeURIComponent(CFG.userKey);
  return `<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>kofi-alert</title>
<link rel="stylesheet" href="file://${d}/css.css">
<link rel="stylesheet" href="file://${d}/stream-alerts.css">
<style>
  :root {
    --alert-background-color: rgba(50, 56, 66, 0.6);
    --alert-text-color: #fff;
  }
  html, body {
    margin: 0; padding: 0; width: 100%; height: 100%;
    background-color: rgba(0,0,0,0);
    overflow: hidden;
  }
  .kofi-alert {
    animation-name: moveToTop, stayAtTop, fadeTop;
    animation-delay: 0s, 6s, 11s;
    animation-duration: 6s, 5s, 1s;
    animation-fill-mode: both;
  }
  /* fallback keyframes if ko-fi's per-user active-style.css is not available */
  @keyframes moveToTop { from { transform: translateY(120%); } to { transform: translateY(0); } }
  @keyframes stayAtTop { from { transform: translateY(0); } to { transform: translateY(0); } }
  @keyframes fadeTop { from { opacity: 1; } to { opacity: 0; } }
  .position { left: 5%; bottom: 5%; }
</style>
</head>
<body>
  <div id="alert-body" style="width:100%; height:100%;">${alertHtml}</div>
  ${ttsAudioTag(ts)}
  <audio id="jingle" src="https://ko-fi.com/sounds/jingle.wav" preload="auto"></audio>
  <script>
    var j = document.getElementById('jingle');
    var t = document.getElementById('tts');
    j && j.play().catch(function(){});
    setTimeout(function(){ if (t) t.play().catch(function(){}); }, 1200);
  </script>
</body>
</html>`;
  function ttsAudioTag() {
    const f = path.join(d, 'tts.wav');
    return fs.existsSync(f) ? `<audio id="tts" src="file://${f}" preload="auto"></audio>` : '';
  }
}

// ---------------------------------------------------------------------------
// Firefox dedicated profile setup (autoplay allowed, no welcome screen)
// ---------------------------------------------------------------------------
async function setupFirefox() {
  const created = await run(CFG.firefoxCmd, ['-CreateProfile', CFG.profileName]);
  log(created.ok ? 'profile created' : `profile error: ${created.stderr || created.err?.message}`);

  const ini = fs.readFileSync(path.join(os.homedir(), '.mozilla', 'firefox', 'profiles.ini'), 'utf8');
  let profileDir = null;
  for (const section of ini.split('\n[')) {
    if (section.includes(`Name=${CFG.profileName}`)) {
      const m = section.match(/Path=(.+)/);
      if (m) profileDir = m[1].trim();
    }
  }
  if (!profileDir) { log('WARN could not find profile dir in profiles.ini'); return; }
  const p = path.join(os.homedir(), '.mozilla', 'firefox', profileDir);
  const prefs = [
    'user_pref("media.autoplay.default", 0);',
    'user_pref("media.autoplay.blocking_policy", 0);',
    'user_pref("browser.aboutwelcome.enabled", false);',
    'user_pref("browser.laterrun.enabled", false);',
    'user_pref("browser.shell.checkDefaultBrowser", false);',
    'user_pref("startup.homepage_welcome_url", "");',
  ].join('\n');
  fs.writeFileSync(path.join(p, 'user.js'), prefs + '\n');
  log(`wrote user.js to ${p}`);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
async function main() {
  const args = process.argv.slice(2);
  fs.mkdirSync(CFG.dataDir, { recursive: true });
  log('ko-fi-popup starting (user ' + CFG.userKey + ')');
  await refreshAssets();

  if (args.includes('--setup')) {
    await setupFirefox();
    log('setup done');
    process.exit(0);
  }

  if (args.includes('--test')) {
    const fake = `<div class="kofi-alert"><div class="position"><div class="sa-box" style="background:var(--alert-background-color);color:var(--alert-text-color);padding:24px 36px;border-radius:12px;font-family:'DM Sans',sans-serif;font-size:28px;font-weight:800;">
      TEST ALERT — <span class="from-name">Gray</span> supported you with <span class="amount">$10</span>!
      <div style="font-size:16px;font-weight:400;margin-top:6px;">this is a local test, not a real donation</div>
    </div></div></div>`;
    await showAlert(fake, 'Test donation from Gray');
    log('test alert fired; window closes in ' + CFG.popupMs / 1000 + 's');
    setTimeout(() => process.exit(0), CFG.popupMs + 3000);
    return;
  }

  await setupFirefox().catch(() => {});
  connectSignalR();
  process.on('SIGTERM', () => process.exit(0));
  process.on('SIGINT', () => process.exit(0));
}

main().catch((e) => {
  log('FATAL: ' + e.stack || e.message);
  process.exit(1);
});