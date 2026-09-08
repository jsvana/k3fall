// k3fall console client: WS protocol handling + canvas waterfall/spectrum
// rendering + click-to-tune. No frameworks, no build step.
(() => {
  'use strict';

  const FLAG_INVERTED = 1;
  const RECONNECT_MS = 2000;

  // ---- DOM ----
  const scopeEl = document.getElementById('scope');
  const spectrumCanvas = document.getElementById('spectrum');
  const fallsCanvas = document.getElementById('falls');
  const vfoline = document.getElementById('vfoline');
  const hoverchip = document.getElementById('hoverchip');
  const freqruler = document.getElementById('freqruler');
  const bannerEl = document.getElementById('banner');
  const vfoaEl = document.getElementById('vfoa');
  const vfobEl = document.getElementById('vfob');
  const chipMode = document.getElementById('chip-mode');
  const chipInv = document.getElementById('chip-inv');
  const indCat = document.getElementById('ind-cat');
  const indSdr = document.getElementById('ind-sdr');
  const clockEl = document.getElementById('clock');
  const teleFft = document.getElementById('tele-fft');
  const teleBin = document.getElementById('tele-bin');
  const teleRows = document.getElementById('tele-rows');
  const teleSpan = document.getElementById('tele-span');
  const teleFps = document.getElementById('tele-fps');

  const sctx = spectrumCanvas.getContext('2d');
  const wctx = fallsCanvas.getContext('2d');
  wctx.imageSmoothingEnabled = false;

  const FLOOR_CSS = 'rgb(26,20,16)';
  const accent = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();

  // 1x-tall scratch canvas used to colormap + rescale each incoming row to
  // the waterfall's current pixel width before scrolling it in.
  const rowCanvas = document.createElement('canvas');
  rowCanvas.height = 1;
  const rowCtx = rowCanvas.getContext('2d');

  // ---- state ----
  let ws = null;
  let reconnectTimer = null;
  let status = null; // last `status` message
  let rig = null; // last `rig` message
  let spanHz = 250000;
  let lastFrame = null; // { centerHz, spanHz, flags }
  let frameCount = 0;
  let bannerTimer = null;
  let resizeTimer = null;
  let lastRulerKey = '';

  let invOverrides = {};
  try {
    invOverrides = JSON.parse(localStorage.getItem('k3fall.inv') || '{}');
  } catch {
    invOverrides = {};
  }

  // ---- heat colormap (mockup's heat(), floor [26,20,16]) ----
  function heat(v) {
    v = Math.min(1, Math.max(0, v));
    if (v < 0.3) {
      const t = v / 0.3;
      return [26 + 44 * t, 20 + 18 * t, 16 + 6 * t];
    }
    if (v < 0.6) {
      const t = (v - 0.3) / 0.3;
      return [70 + 134 * t, 38 + 62 * t, 22 + 8 * t];
    }
    if (v < 0.85) {
      const t = (v - 0.6) / 0.25;
      return [204 + 45 * t, 100 + 90 * t, 30 + 60 * t];
    }
    const t = (v - 0.85) / 0.15;
    return [249, 190 + 55 * t, 90 + 150 * t];
  }
  const HEAT_LUT = new Uint8ClampedArray(256 * 3);
  for (let i = 0; i < 256; i++) {
    const [r, g, b] = heat(i / 255);
    HEAT_LUT[i * 3] = r;
    HEAT_LUT[i * 3 + 1] = g;
    HEAT_LUT[i * 3 + 2] = b;
  }

  // ---- frequency formatting ("14.032.50" style) ----
  function formatFreqParts(hz) {
    hz = Math.max(0, Math.round(hz));
    const mhz = Math.floor(hz / 1e6);
    let rem = hz % 1e6;
    const khz = Math.floor(rem / 1000);
    rem = rem % 1000;
    const hzTens = Math.floor(rem / 10);
    return { mhz, khz: String(khz).padStart(3, '0'), hz: String(hzTens).padStart(2, '0') };
  }
  function formatFreqText(hz) {
    const p = formatFreqParts(hz);
    return `${p.mhz}.${p.khz}.${p.hz}`;
  }
  function renderVfoA(hz) {
    const p = formatFreqParts(hz);
    vfoaEl.innerHTML = `${p.mhz}.<span class="khz">${p.khz}</span><span class="hz">.${p.hz}</span>`;
  }

  // ---- frequency <-> pixel mapping ----
  function currentBand() {
    const hz = lastFrame ? lastFrame.centerHz : rig ? rig.vfo_a_hz : 0;
    return Math.floor(hz / 1e6);
  }
  // True when the incoming row's bins run high->low (hardware-inverted IF
  // spectrum, possibly corrected by the user's INV override). The row gets
  // pixel-mirrored before painting, so on-screen RF ALWAYS increases
  // left->right and every mapping below stays direction-free.
  function rowReversed() {
    if (!lastFrame) return false;
    const hardwareInverted = (lastFrame.flags & FLAG_INVERTED) !== 0;
    const override = invOverrides[currentBand()] === true;
    return override ? !hardwareInverted : hardwareInverted;
  }
  function freqAt(xCss, cssWidth) {
    if (!lastFrame || cssWidth <= 0) return lastFrame ? lastFrame.centerHz : 0;
    return lastFrame.centerHz + (xCss / cssWidth - 0.5) * lastFrame.spanHz;
  }
  function fracForFreq(freqHz) {
    return 0.5 + (freqHz - lastFrame.centerHz) / lastFrame.spanHz;
  }

  function updateInvChipUI() {
    chipInv.classList.toggle('on', invOverrides[currentBand()] === true);
  }
  chipInv.addEventListener('click', () => {
    const band = currentBand();
    invOverrides[band] = !invOverrides[band];
    localStorage.setItem('k3fall.inv', JSON.stringify(invOverrides));
    updateInvChipUI();
    updateVfoLine();
    lastRulerKey = '';
    buildRuler();
  });

  // ---- VFO cursor line ----
  function updateVfoLine() {
    if (!lastFrame || !rig) {
      vfoline.hidden = true;
      return;
    }
    const frac = fracForFreq(rig.vfo_a_hz);
    if (!isFinite(frac) || frac < 0 || frac > 1) {
      vfoline.hidden = true;
      return;
    }
    vfoline.hidden = false;
    vfoline.style.left = `${frac * 100}%`;
  }

  // ---- frequency ruler ----
  const RULER_STEPS = [
    500, 1000, 2000, 5000, 10000, 15000, 20000, 25000, 50000, 100000, 150000, 200000, 250000,
    500000, 1000000,
  ];
  function niceStep(raw) {
    for (const s of RULER_STEPS) if (s >= raw) return s;
    return RULER_STEPS[RULER_STEPS.length - 1];
  }
  function buildRulerInner() {
    freqruler.innerHTML = '';
    if (!lastFrame) return;
    const { centerHz, spanHz: span } = lastFrame;
    const step = niceStep(span / 6);
    const lo = centerHz - 0.5 * span;
    const hi = centerHz + 0.5 * span;
    const first = Math.ceil(lo / step) * step;
    for (let f = first; f <= hi + 1e-6; f += step) {
      const frac = fracForFreq(f);
      const el = document.createElement('span');
      el.textContent = (f / 1e6).toFixed(3);
      el.style.left = `${frac * 100}%`;
      freqruler.appendChild(el);
    }
  }
  function buildRuler() {
    buildRulerInner();
    renderSpots();
  }

  function maybeRebuildRuler() {
    if (!lastFrame) return;
    const key = `${lastFrame.centerHz}|${lastFrame.spanHz}|${rowReversed()}`;
    if (key === lastRulerKey) return;
    lastRulerKey = key;
    buildRuler();
  }

  // ---- canvas sizing ----
  // Only touches `cv.width`/`cv.height` (which wipes the canvas's pixel
  // buffer as a side effect) when the device-pixel size actually changed.
  // Headless Chrome fires a spurious extra `resize` event right before a
  // `--screenshot` capture; without this guard that event would blank an
  // already-painted waterfall with no time left for new rows to refill it.
  function sizeCanvas(cv) {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(cv.clientWidth * dpr));
    const h = Math.max(1, Math.round(cv.clientHeight * dpr));
    if (cv.width === w && cv.height === h) return false;
    cv.width = w;
    cv.height = h;
    return true;
  }
  function resizeCanvases() {
    const specChanged = sizeCanvas(spectrumCanvas);
    const fallsChanged = sizeCanvas(fallsCanvas);
    if (specChanged) {
      sctx.fillStyle = FLOOR_CSS;
      sctx.fillRect(0, 0, spectrumCanvas.width, spectrumCanvas.height);
    }
    if (fallsChanged) {
      wctx.fillStyle = FLOOR_CSS;
      wctx.fillRect(0, 0, fallsCanvas.width, fallsCanvas.height);
    }
    return specChanged || fallsChanged;
  }
  resizeCanvases();

  window.addEventListener('resize', () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      if (resizeCanvases()) sendView();
    }, 250);
  });

  // ---- painting ----
  function paintRow(bins) {
    const n = bins.length;
    if (n === 0) return;
    if (rowCanvas.width !== n) rowCanvas.width = n;
    const img = rowCtx.createImageData(n, 1);
    for (let i = 0; i < n; i++) {
      const v = bins[i];
      img.data[i * 4] = HEAT_LUT[v * 3];
      img.data[i * 4 + 1] = HEAT_LUT[v * 3 + 1];
      img.data[i * 4 + 2] = HEAT_LUT[v * 3 + 2];
      img.data[i * 4 + 3] = 255;
    }
    rowCtx.putImageData(img, 0, 0);

    const w = fallsCanvas.width;
    const h = fallsCanvas.height;
    if (h > 1) wctx.drawImage(fallsCanvas, 0, 0, w, h - 1, 0, 1, w, h - 1);
    // Fewer bins than pixels (deep zoom): interpolate the stretch so the
    // display reads as a continuous spectrum instead of blocky bins.
    wctx.imageSmoothingEnabled = n < w;
    wctx.drawImage(rowCanvas, 0, 0, n, 1, 0, 0, w, 1);
    wctx.imageSmoothingEnabled = false;

    drawSpectrum(bins);
    frameCount++;
  }

  function drawSpectrum(bins) {
    const w = spectrumCanvas.width;
    const h = spectrumCanvas.height;
    const n = bins.length;
    sctx.fillStyle = FLOOR_CSS;
    sctx.fillRect(0, 0, w, h);
    sctx.strokeStyle = accent || 'orange';
    sctx.lineWidth = Math.max(1, 1.5 * (window.devicePixelRatio || 1));
    sctx.beginPath();
    for (let i = 0; i < n; i++) {
      const x = n > 1 ? (i / (n - 1)) * w : 0;
      const y = h - (bins[i] / 255) * (h - 6) - 3;
      if (i === 0) sctx.moveTo(x, y);
      else sctx.lineTo(x, y);
    }
    sctx.stroke();
  }

  // ---- websocket ----
  function wsUrl() {
    const proto = location.protocol === 'https:' ? 'wss://' : 'ws://';
    return `${proto}${location.host}/ws`;
  }
  function sendView() {
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    const width = Math.min(4096, fallsCanvas.width || 1);
    ws.send(JSON.stringify({ type: 'view', width, span_hz: Math.round(spanHz), center_offset_hz: 0 }));
  }
  function updateSpanTele() {
    if (zoomWin) updateZoombar();
    teleSpan.textContent = (spanHz / 1000).toFixed(1);
  }

  let spots = [];
  const spotLayer = document.getElementById('spotlayer');
  const zoomWin = document.getElementById('zoomwin');
  let powerOn = false;
  const btnPwr = document.getElementById('btn-pwr');
  btnPwr.addEventListener('click', () => {
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    btnPwr.classList.add('pending');
    ws.send(JSON.stringify({ type: 'power', on: !powerOn }));
  });

  function renderSpots() {
    spotLayer.textContent = '';
    if (!lastFrame) return;
    for (const s of spots) {
      const frac = fracForFreq(s.freq_hz);
      if (frac < 0 || frac > 1) continue;
      const el = document.createElement('span');
      el.className = 'smark ' + (s.source === 'pota' ? 'pota' : 'rbn') +
        (s.age_s > 300 ? ' stale' : '');
      el.style.left = (frac * 100).toFixed(2) + '%';
      el.textContent = s.callsign;
      if (s.label) {
        const extra = document.createElement('i');
        extra.textContent = s.label;
        el.appendChild(extra);
      } else if (s.snr_db != null) {
        const extra = document.createElement('i');
        extra.textContent = s.snr_db + 'dB';
        el.appendChild(extra);
      }
      el.title = s.callsign + ' \u00b7 ' + formatFreqText(s.freq_hz) +
        (s.label ? ' \u00b7 ' + s.label : '') + ' \u00b7 ' + Math.round(s.age_s / 60) + 'm ago';
      el.addEventListener('click', (ev) => {
        ev.stopPropagation();
        sendTune(s.freq_hz);
      });
      spotLayer.appendChild(el);
    }
  }

  function handleStatus(msg) {
    status = msg;
    spanHz = msg.sample_rate;
    teleFft.textContent = msg.fft_size;
    teleBin.textContent = msg.bin_hz.toFixed(2);
    teleRows.textContent = msg.rows_per_sec;
    updateSpanTele();
    indSdr.classList.toggle('on', !!msg.sdr_ok);
    indCat.classList.toggle('on', !!msg.cat_ok);
    resizeCanvases();
    sendView();
  }
  function handleRig(msg) {
    rig = msg;
    renderVfoA(rig.vfo_a_hz);
    vfobEl.textContent = formatFreqText(rig.vfo_b_hz);
    chipMode.textContent = rig.mode;
    indCat.classList.toggle('on', !!rig.cat_connected);
    updateInvChipUI();
    updateVfoLine();
  }
  function handleJson(text) {
    let msg;
    try {
      msg = JSON.parse(text);
    } catch {
      return;
    }
    if (msg.type === 'status') handleStatus(msg);
    else if (msg.type === 'rig') handleRig(msg);
    else if (msg.type === 'error') showBanner(msg.message);
    else if (msg.type === 'spots') { spots = msg.spots || []; renderSpots(); }
    else if (msg.type === 'power') {
      btnPwr.classList.remove('pending');
      if (msg.ok) { powerOn = msg.on; btnPwr.classList.toggle('on', powerOn); }
      else showBanner('power command failed');
    }
  }

  function handleRowFrame(buf) {
    const dv = new DataView(buf);
    if (buf.byteLength < 19) return;
    const centerHz = Number(dv.getBigUint64(4, true));
    const spanHzHdr = dv.getUint32(12, true);
    const bins = dv.getUint16(16, true);
    const flags = dv.getUint8(18);
    if (buf.byteLength < 19 + bins) return;
    let payload = new Uint8Array(buf, 19, bins);
    lastFrame = { centerHz, spanHz: spanHzHdr, flags };
    if (rowReversed()) payload = payload.slice().reverse();
    paintRow(payload);
    updateVfoLine();
    maybeRebuildRuler();
  }

  function scheduleReconnect() {
    document.body.classList.add('disconnected');
    indCat.classList.remove('on');
    indSdr.classList.remove('on');
    hoverchip.hidden = true;
    vfoline.hidden = true;
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, RECONNECT_MS);
  }

  function connect() {
    ws = new WebSocket(wsUrl());
    ws.binaryType = 'arraybuffer';
    ws.addEventListener('open', () => {
      document.body.classList.remove('disconnected');
    });
    ws.addEventListener('message', (event) => {
      if (typeof event.data === 'string') handleJson(event.data);
      else handleRowFrame(event.data);
    });
    ws.addEventListener('close', scheduleReconnect);
    ws.addEventListener('error', () => {
      ws.close();
    });
  }

  // ---- interaction: zoom / tune / hover ----
  function updateZoombar() {
    if (!status) return;
    const frac = Math.min(1, spanHz / status.sample_rate);
    zoomWin.style.width = `${(frac * 100).toFixed(2)}%`;
    zoomWin.style.left = `${((1 - frac) * 50).toFixed(2)}%`;
  }

  let viewSendTimer = null;
  function scheduleSendView() {
    if (viewSendTimer) return;
    viewSendTimer = setTimeout(() => {
      viewSendTimer = null;
      sendView();
    }, 80);
  }

  scopeEl.addEventListener(
    'wheel',
    (e) => {
      e.preventDefault();
      if (!status) return;
      // Proportional zoom: ~300px of wheel travel per octave, so trackpads
      // glide instead of slamming between the limits.
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 100 : 1;
      let factor = Math.pow(2, (e.deltaY * unit) / 300);
      factor = Math.min(2, Math.max(0.5, factor));
      const minSpan = Math.max(1, status.bin_hz * 128);
      const maxSpan = status.sample_rate;
      const next = Math.min(maxSpan, Math.max(minSpan, Math.round(spanHz * factor)));
      if (next !== spanHz) {
        spanHz = next;
        updateSpanTele();
        updateZoombar();
        scheduleSendView();
      }
    },
    { passive: false },
  );

  function sendTune(freqHz) {
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    const rounded = Math.round(freqHz / 10) * 10;
    ws.send(JSON.stringify({ type: 'tune', freq_hz: rounded }));
  }

  scopeEl.addEventListener('click', (e) => {
    if (!lastFrame) return;
    const rect = scopeEl.getBoundingClientRect();
    const x = e.clientX - rect.left;
    sendTune(freqAt(x, rect.width));
  });

  scopeEl.addEventListener('mousemove', (e) => {
    if (!lastFrame) {
      hoverchip.hidden = true;
      return;
    }
    const rect = scopeEl.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    if (x < 0 || x > rect.width || y < 0 || y > rect.height) {
      hoverchip.hidden = true;
      return;
    }
    const freq = freqAt(x, rect.width);
    hoverchip.textContent = formatFreqText(freq);
    hoverchip.hidden = false;
    hoverchip.style.left = `${x}px`;
    hoverchip.style.top = `${Math.max(4, y - 28)}px`;
  });
  scopeEl.addEventListener('mouseleave', () => {
    hoverchip.hidden = true;
  });

  // ---- banner ----
  function showBanner(message) {
    bannerEl.textContent = message;
    bannerEl.hidden = false;
    clearTimeout(bannerTimer);
    bannerTimer = setTimeout(() => {
      bannerEl.hidden = true;
    }, 4000);
  }

  // ---- clock ----
  function tickClock() {
    const d = new Date();
    const hh = String(d.getUTCHours()).padStart(2, '0');
    const mm = String(d.getUTCMinutes()).padStart(2, '0');
    const ss = String(d.getUTCSeconds()).padStart(2, '0');
    clockEl.textContent = `${hh}:${mm}:${ss}z`;
  }
  setInterval(tickClock, 1000);
  tickClock();

  // ---- fps counter ----
  setInterval(() => {
    teleFps.textContent = frameCount;
    frameCount = 0;
  }, 1000);

  connect();
})();
