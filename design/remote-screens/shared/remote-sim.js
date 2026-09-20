/* Remote Screens prototype engine, shared by desktop.html, ios.html and android.html.
 *
 * It simulates one remote Mac ("mac-studio") whose screen keeps changing: a clock, streaming
 * terminal output, an agent waiting for approval, a playing video, a chat on a second display.
 * Every change is turned into what Multiplex would send: skipped pixels, 64×64 tiles, cached
 * tiles, terminal text, or a motion stream. A network profile drives the degradation steps.
 * Pages mount views of the scene and bind their own UI to the events below.
 */
(function () {
  'use strict';

  const RS = (window.RS = {});
  const handlers = {};
  RS.on = (name, fn) => { (handlers[name] = handlers[name] || []).push(fn); };
  const emit = (name, data) => (handlers[name] || []).forEach(fn => fn(data));
  RS.emit = emit;

  /* ------------------------------------------------------------------ network profiles */
  const NETS = RS.NETS = {
    good:   { label: 'Good Wi-Fi',     route: 'Direct', medium: 'Wi-Fi 6',        rtt: 38,  loss: 0.2,  capKbps: 42000, target: 0 },
    cafe:   { label: 'Busy café',      route: 'Direct', medium: 'Public Wi-Fi',   rtt: 96,  loss: 1.1,  capKbps: 5000,  target: 1 },
    weak:   { label: 'Weak cellular',  route: 'Relay',  medium: 'Cellular',       rtt: 310, loss: 6.1,  capKbps: 240,   target: 3 },
    barely: { label: 'Barely there',   route: 'Relay',  medium: 'Cellular, 1 bar', rtt: 540, loss: 10.4, capKbps: 110,   target: 5 }
  };
  const STEP_ON = [null, 'Paused sharpening of pictures', 'Paused areas outside your view', 'Slowed to 8 updates per second', 'Lower detail on pictures', 'Half resolution'];
  const STEP_OFF = [null, 'Resumed sharpening of pictures', 'Resumed areas outside your view', 'Back to 60 updates per second', 'Full detail on pictures again', 'Full resolution again'];
  RS.STEPS = STEP_ON;

  /* ------------------------------------------------------------------ model */
  const REQUESTS = ['cargo bench -p termirust-screen-codec', 'git push origin screen-codec', 'cargo test --workspace'];
  const M = RS.model = {
    secs: 14 * 3600 + 32 * 60 + 5,
    net: 'good', link: 'up', step: 0,
    video: true, videoPos: 0.38, videoAngle: 0,
    throughput: 48.2, chart: [100, 90, 94, 70, 76, 50, 58, 40, 46, 28, 34],
    pane1: [
      '<span class="m">$</span> cargo watch -x "test -p termirust-screen-codec"',
      '   <span class="g">Compiling</span> termirust-screen-codec v0.1.0',
      '    <span class="g">Finished</span> `test` profile in 4.21s',
      'running 48 tests',
      'test tile::solid_roundtrip ... <span class="g">ok</span>',
      'test tile::lossless_text_roundtrip ... <span class="g">ok</span>',
      'test cache::missing_nack_resends ... <span class="g">ok</span>',
      'test scroll::detects_three_row_offset ... <span class="g">ok</span>',
      'test region::promotes_after_300ms ... <span class="g">ok</span>',
      'test result: <span class="g">ok</span>. 48 passed; 0 failed'
    ],
    typed: '',
    agent: 'waiting', agentLines: [], request: 0, device: 'this Mac',
    chat: [
      { who: 'Priya', c: '#E5787A', text: 'relay-02 is back, 40 ms from Berlin', at: '14:21' },
      { who: 'Sam', c: '#6CCB8F', text: 'Deploying relay-host 0.9.3 to staging', at: '14:26' },
      { who: 'CI', c: '#74A7F2', text: 'screen-codec · 48 tests passed', at: '14:30' }
    ],
    linux: [],
    away: { tiles: 0, text: 0 },
    samples: new Array(80).fill(1400),
    v: { clock: 0, pane1: 0, pane2: 0, video: 0, chart: 0, chat: 0, linux: 0 }
  };
  for (let i = 0; i < 44; i++) M.linux.push(linuxLine(i));

  function linuxLine(i) {
    return '<span class="m">[14:' + String((10 + i) % 60).padStart(2, '0') + ':' + String((i * 7) % 60).padStart(2, '0') + ']</span> <span class="g">INFO</span> relay: forwarded ' + (1200 + i * 37) + ' frames to peer ' + ['a3f1', '9c02', '77be'][i % 3];
  }
  const clockText = () => {
    const s = M.secs % 86400;
    return String(Math.floor(s / 3600)).padStart(2, '0') + ':' + String(Math.floor(s / 60) % 60).padStart(2, '0') + ':' + String(s % 60).padStart(2, '0');
  };
  RS.clock = clockText;

  const state = RS.state = () => {
    const n = NETS[M.net];
    const up = M.link === 'up';
    const path = up && M.video && M.step < 3 ? 'motion' : 'tile';
    return {
      net: M.net, profile: n, link: M.link, connected: up, step: M.step, path,
      rtt: n.rtt, loss: n.loss, route: n.route, medium: n.medium,
      ups: !up ? 0 : M.step >= 3 ? 8 : 60,
      kbps: up ? M.samples[M.samples.length - 1] : 0,
      reused: [86, 88, 90, 91, 92, 93][M.step]
    };
  };
  RS.fmtKbps = k => k >= 1000 ? (k / 1000).toFixed(1) + ' Mbps' : Math.max(0, Math.round(k)) + ' kbps';

  /* ------------------------------------------------------------------ geometry */
  const GAP = 40;
  const LAYOUTS = RS.LAYOUTS = {
    all:     { w: 1512 + GAP + 1170, h: 982, displays: [['main', 0, 0], ['builtin', 1512 + GAP, 982 - 760]] },
    studio:  { w: 1512, h: 982, displays: [['main', 0, 0]] },
    builtin: { w: 1170, h: 760, displays: [['builtin', 0, 0]] },
    linux:   { w: 1512, h: 982, displays: [['linux', 0, 0]] }
  };
  const R = {
    clock: { d: 'main', x: 1296, y: 0, w: 210, h: 26 },
    pane1: { d: 'main', x: 891, y: 91, w: 294, h: 418 },
    pane2: { d: 'main', x: 1186, y: 91, w: 293, h: 418 },
    video: { d: 'main', x: 885, y: 605, w: 351, h: 281 },
    chart: { d: 'main', x: 485, y: 605, w: 386, h: 281 },
    chat:  { d: 'builtin', x: 40, y: 560, w: 700, h: 90 },
    bclock: { d: 'builtin', x: 960, y: 0, w: 200, h: 26 }
  };
  RS.RECTS = R;

  /* ------------------------------------------------------------------ scene markup */
  const CODE = [
    '<span class="c">// Pick how one 64×64 tile travels.</span>',
    '<span class="k">pub fn</span> <span class="f">classify</span>(tile: &amp;<span class="t">Tile</span>) -&gt; <span class="t">TileMode</span> {',
    '    <span class="k">let</span> colors = tile.<span class="f">distinct_colors</span>(<span class="n">65</span>);',
    '    <span class="k">if</span> colors == <span class="n">1</span> {',
    '        <span class="k">return</span> <span class="t">TileMode</span>::<span class="t">Solid</span>(tile.<span class="f">pixel</span>(<span class="n">0</span>, <span class="n">0</span>));',
    '    }',
    '    <span class="k">if</span> colors &lt;= <span class="n">64</span> || tile.<span class="f">edge_density</span>() &gt; <span class="n">0.18</span> {',
    '        <span class="k">return</span> <span class="t">TileMode</span>::<span class="t">Lossless</span>;',
    '    }',
    '    <span class="t">TileMode</span>::<span class="t">Lossy</span> { quality: <span class="n">62</span> }',
    '}',
    '',
    '<span class="k">pub fn</span> <span class="f">detect_scroll</span>(prev: &amp;<span class="t">HashGrid</span>, next: &amp;<span class="t">HashGrid</span>) -&gt; <span class="t">Option</span>&lt;<span class="t">Move</span>&gt; {',
    '    <span class="k">for</span> offset <span class="k">in</span> [<span class="n">-3</span>, <span class="n">-2</span>, <span class="n">-1</span>, <span class="n">1</span>, <span class="n">2</span>, <span class="n">3</span>] {',
    '        <span class="k">if</span> next.<span class="f">matches_shifted</span>(prev, offset) {',
    '            <span class="k">return</span> <span class="t">Some</span>(<span class="t">Move</span>::<span class="f">vertical</span>(offset));',
    '        }',
    '    }',
    '    <span class="t">None</span>',
    '}',
    '',
    '<span class="k">#[test]</span>',
    '<span class="k">fn</span> <span class="f">text_tile_is_lossless</span>() {',
    '    <span class="k">let</span> tile = <span class="t">Tile</span>::<span class="f">from_fixture</span>(<span class="s">"editor-line.bgra"</span>);'
  ].map((l, i) => '<span class="ln">' + (i + 41) + '</span>' + l).join('\n');
  const DOCK = ['#3A7BD5', '#5AC8FA', '#34C759', '#FF9F0A', '#AF52DE', '#2C2C2E', '#FF375F', '#8E8E93'];

  function mainDisplay(x, y) {
    return '<div class="rd" data-display="main" style="left:' + x + 'px;top:' + y + 'px;width:1512px;height:982px">' +
      '<div class="rd-menubar"><b>■</b><b>Zed</b><span>File</span><span>Edit</span><span>Selection</span><span>View</span><span>Go</span><span>Window</span><span>Help</span><div class="r"><span>Wi-Fi</span><span>100%</span><span>Tue 15 Sep <span data-live="clock"></span></span></div></div>' +
      '<div class="rd-win" style="left:36px;top:52px;width:830px;height:560px"><div class="rd-tb"><i></i><i></i><i></i><span>termirust — classify.rs</span></div><div class="rd-editor-body"><div class="rd-tree"><div>termirust</div><div class="d2">crates</div><div class="d3">termirust-screen-capture</div><div class="d3 sel">termirust-screen-codec</div><div class="d3">termirust-screen-host</div><div class="d3">termirust-screen-transport</div><div class="d3">termirust-controller-listener</div><div class="d3">termirust-desktop</div><div class="d2">docs</div><div class="d2">design</div><div class="d2">tests</div><div>Cargo.toml</div></div><pre class="rd-code">' + CODE + '</pre></div></div>' +
      '<div class="rd-win term" style="left:890px;top:60px;width:590px;height:450px"><div class="rd-tb"><i></i><i></i><i></i><span>Multiplex</span></div><div class="rd-term-body"><div class="rd-pane"><div class="hd">zsh — termirust</div><div data-live="pane1"></div></div><div class="rd-pane"><div class="hd">agent — screen-codec worktree</div><div data-live="pane2"></div></div></div></div>' +
      '<div class="rd-win" style="left:470px;top:560px;width:780px;height:340px"><div class="rd-tb"><i></i><i></i><i></i><span>Relay metrics — Safari</span></div><div class="rd-browser-body"><div class="rd-panel"><h6>Throughput, last hour</h6><div class="big"><span data-live="throughput"></span> Mbps</div><svg viewBox="0 0 300 120" width="100%" height="150"><path data-live="chart" d="" fill="none" stroke="#2A64C8" stroke-width="2.5"/><path d="M0 119.5H300" stroke="#DADDE2"/></svg></div><div class="rd-video" data-hit="video"><div class="frame"></div><div class="shade"></div><div class="play" data-live="play">❚❚</div><div class="bar"><i data-live="videobar"></i></div></div></div></div>' +
      '<div class="rd-dock">' + DOCK.map(c => '<i style="background:' + c + '"></i>').join('') + '</div></div>';
  }
  function builtinDisplay(x, y) {
    return '<div class="rd builtin" data-display="builtin" style="left:' + x + 'px;top:' + y + 'px;width:1170px;height:760px">' +
      '<div class="rd-menubar"><b>■</b><b>Messages</b><span>File</span><span>Edit</span><span>View</span><div class="r"><span data-live="clock"></span></div></div>' +
      '<div class="rd-chat" style="left:40px;top:60px;width:700px;height:620px"><div class="rd-tb" style="background:#24222C;border-color:#34313F"><i></i><i></i><i></i><span># relay-ops</span></div><div class="msgs" data-live="chat"></div><div class="compose">Message #relay-ops</div></div>' +
      '<div class="rd-chat" style="left:780px;top:60px;width:350px;height:300px;padding:14px 16px;display:grid;align-content:start;gap:10px"><b style="font-size:15px">Today</b><div style="color:#B9B6C4;font-size:13px;line-height:20px">15:00 Relay capacity review<br>16:30 Screen codec design sync<br>18:00 On-call handover</div></div>' +
      '</div>';
  }
  function linuxDisplay(x, y) {
    return '<div class="rd linux" data-display="linux" style="left:' + x + 'px;top:' + y + 'px;width:1512px;height:982px">' +
      '<div class="rd-menubar"><span>Activities</span><span style="margin:0 auto" data-live="clock"></span></div>' +
      '<div class="rd-win term" style="left:12px;top:36px;width:740px;height:934px;border-radius:6px"><div class="rd-tb"><span>ops@build-box: ~/relay</span></div><div class="rd-term-body"><div class="rd-pane" style="padding-top:10px"><div data-live="linux"></div></div></div></div>' +
      '<div class="rd-win term" style="left:760px;top:36px;width:740px;height:934px;border-radius:6px"><div class="rd-tb"><span>htop</span></div><div class="rd-term-body"><div class="rd-pane" style="padding-top:10px;justify-content:flex-start">  CPU[<span class="g">||||||||||||</span><span class="y">|||</span>        38.4%]\n  Mem[<span class="g">|||||||||||||||||</span><span class="b">||||</span>  11.2G/31.3G]\n  Swp[                         0K/2.00G]\n\n  <span style="background:#6CCB8F;color:#17191D">  PID USER      CPU%  MEM%  COMMAND                  </span>\n  4121 ops       22.1   3.4  termirust relay-host run\n  3380 ops        9.8   1.2  cargo build --release\n   912 root       2.3   0.4  sshd: ops@pts/2\n  1207 ops        1.1   0.9  tmux: server</div></div></div></div>';
  }
  const DISPLAY_HTML = { main: mainDisplay, builtin: builtinDisplay, linux: linuxDisplay };

  /* ------------------------------------------------------------------ live parts */
  function pane1HTML() {
    const lines = M.pane1.slice(-19).map(l => '<span class="ln-row">' + l + '</span>').join('');
    return lines + '<span class="ln-row"><span class="m">$</span> ' + esc(M.typed) + '<span class="cur"></span></span>';
  }
  function pane2HTML() {
    const head = ['<span class="b">●</span> Read src/cache.rs', '<span class="b">●</span> Edit src/scroll.rs  <span class="g">+42</span> <span class="r">−7</span>', '  Scroll detection now tests ±3', '  tile offsets against the previous', '  frame\'s column hashes.', ''];
    let body;
    if (M.agent === 'waiting') {
      body = ['<span class="y">▲ Waiting for approval</span>', '  Run: ' + REQUESTS[M.request], '', '  <span data-hit="approve"><span class="b">›</span> 1  Yes</span>', '    2  Yes, don\'t ask again', '  <span data-hit="deny">  3  No</span>'];
    } else {
      body = M.agentLines;
    }
    return head.concat(body).map(l => '<span class="ln-row">' + l + '</span>').join('');
  }
  function chatHTML() {
    return M.chat.slice(-7).map(m => '<div class="msg"><i style="background:' + m.c + '"></i><div><b>' + m.who + '</b><small>' + m.at + '</small><br>' + m.text + '</div></div>').join('');
  }
  const chartPath = () => M.chart.map((v, i) => (i ? 'L' : 'M') + (i * 30) + ' ' + v).join(' ');
  const esc = s => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');

  /* ------------------------------------------------------------------ hosts (mounted views) */
  const hosts = [];
  RS.hosts = hosts;

  RS.mount = function (el, opts) {
    opts = Object.assign({ layout: 'studio', fit: 'cover', overlay: false, pointer: false, computer: 'mac-studio', receiver: false }, opts || {});
    el.classList.add('rs-host', opts.fit === 'interactive' ? 'interactive' : 'static');
    const h = { el, opts, painted: {}, s: 1, tx: 0, ty: 0, fit: true, trackpad: false, cursor: { x: 1200, y: 360 }, pointerOn: !!opts.pointer, listeners: {} };
    h.on = (n, fn) => { (h.listeners[n] = h.listeners[n] || []).push(fn); };
    h.emit = (n, d) => (h.listeners[n] || []).forEach(fn => fn(d));
    h.canvas = document.createElement('div');
    h.canvas.className = 'rs-canvas';
    el.prepend(h.canvas);

    h.setLayout = name => {
      h.layout = name; const L = LAYOUTS[name];
      h.canvas.style.width = L.w + 'px'; h.canvas.style.height = L.h + 'px';
      h.canvas.innerHTML = L.displays.map(([d, x, y]) => DISPLAY_HTML[d](x, y)).join('') +
        '<div class="rs-overlay" style="width:' + L.w + 'px;height:' + L.h + 'px"></div>' +
        '<svg class="rs-pointer" viewBox="0 0 22 22" hidden><path d="M3 2v16.2l4.1-3.8 2.8 6.2 2.7-1.2-2.8-6.1h6z" fill="#fff" stroke="#000" stroke-width="1.2"/></svg>';
      h.overlay = h.canvas.querySelector('.rs-overlay');
      h.pointerEl = h.canvas.querySelector('.rs-pointer');
      h.painted = {};
      paint(h, true);
      h.setOverlay(h.opts.overlay);
      h.relayout(true);
    };
    h.origin = d => { const e = LAYOUTS[h.layout].displays.find(x => x[0] === d); return e ? { x: e[1], y: e[2] } : null; };
    h.size = () => ({ w: el.clientWidth, h: el.clientHeight });
    h.fitScale = () => { const L = LAYOUTS[h.layout], z = h.size(); return Math.min(z.w / L.w, z.h / L.h); };
    h.coverScale = () => { const L = LAYOUTS[h.layout], z = h.size(); return Math.max(z.w / L.w, z.h / L.h); };
    h.pageScale = () => { const r = el.getBoundingClientRect(); return el.offsetWidth ? r.width / el.offsetWidth : 1; };

    h.apply = () => {
      h.canvas.style.transform = 'translate(' + h.tx + 'px,' + h.ty + 'px) scale(' + h.s + ')';
      if (h.pointerEl) { h.pointerEl.style.transform = 'translate(' + h.cursor.x + 'px,' + h.cursor.y + 'px) scale(' + (1 / h.s) + ')'; h.pointerEl.hidden = !h.pointerOn; }
      el.classList.toggle('pannable', h.opts.fit === 'interactive' && !h.trackpad && h.canPan());
      h.emit('view', h.view());
    };
    h.canPan = () => { const L = LAYOUTS[h.layout], z = h.size(); return L.w * h.s > z.w + 1 || L.h * h.s > z.h + 1; };
    h.clamp = () => {
      const L = LAYOUTS[h.layout], z = h.size();
      const cw = L.w * h.s, ch = L.h * h.s;
      h.tx = cw <= z.w ? (z.w - cw) / 2 : Math.min(0, Math.max(z.w - cw, h.tx));
      h.ty = ch <= z.h ? (z.h - ch) / 2 : Math.min(0, Math.max(z.h - ch, h.ty));
    };
    h.view = () => { const z = h.size(); return { x: -h.tx / h.s, y: -h.ty / h.s, w: z.w / h.s, h: z.h / h.s, s: h.s, fit: h.fit }; };
    h.relayout = first => {
      if (h.opts.fit === 'cover') { h.s = h.coverScale(); h.tx = 0; h.ty = 0; h.fit = true; }
      else if (h.opts.fit === 'contain' || h.fit) { h.s = h.fitScale(); h.fit = true; h.clamp(); }
      else { if (first && h.opts.center) h.centerOn(h.opts.center[0], h.opts.center[1], true); h.clamp(); }
      h.apply();
    };
    h.setZoom = (z, clientX, clientY) => {
      const r = el.getBoundingClientRect(), ps = h.pageScale(), sz = h.size();
      const lx = clientX == null ? sz.w / 2 : (clientX - r.left) / ps;
      const ly = clientY == null ? sz.h / 2 : (clientY - r.top) / ps;
      const rx = (lx - h.tx) / h.s, ry = (ly - h.ty) / h.s;
      if (z === 'fit') { h.fit = true; h.s = h.fitScale(); h.clamp(); }
      else { h.fit = false; h.s = Math.max(h.fitScale(), Math.min(4, z)); if (Math.abs(h.s - h.fitScale()) < 0.001) h.fit = true; h.tx = lx - rx * h.s; h.ty = ly - ry * h.s; h.clamp(); }
      h.apply();
      h.emit('zoom', h.s);
    };
    h.zoomBy = (f, cx, cy) => h.setZoom(h.s * f, cx, cy);
    h.centerOn = (rx, ry, silent) => {
      const z = h.size(); h.tx = z.w / 2 - rx * h.s; h.ty = z.h / 2 - ry * h.s; h.clamp(); if (!silent) h.apply();
    };
    h.setOverlay = on => {
      h.opts.overlay = on; h.overlay.classList.toggle('grid', on); h.overlay.innerHTML = '';
      if (on) drawZones(h);
    };
    h.setTrackpad = on => { h.trackpad = on; h.pointerOn = on || !!h.opts.pointer; el.classList.toggle('trackpad', on); h.apply(); };
    h.setPointer = on => { h.opts.pointer = on; h.pointerOn = on || h.trackpad; h.apply(); };
    h.frozen = () => h.opts.computer === 'mac-studio' && M.link !== 'up';

    if (opts.fit === 'interactive') bindInteraction(h);
    new ResizeObserver(() => h.relayout()).observe(el);
    hosts.push(h);
    h.setLayout(opts.layout);
    if (opts.fit === 'interactive' && opts.zoom && opts.zoom !== 'fit') { h.fit = false; h.s = opts.zoom; if (opts.center) h.centerOn(opts.center[0], opts.center[1], true); h.clamp(); h.apply(); }
    return h;
  };

  function paint(h, force) {
    const frozen = h.frozen();
    h.el.classList.toggle('frozen', frozen);
    if (h.opts.receiver) {
      h.el.classList.toggle('soft-pictures', M.step >= 4);
      h.el.classList.toggle('half-res', M.step >= 5);
    }
    if (frozen && !force) return;
    const c = h.canvas, p = h.painted;
    const set = (key, sel, fn) => {
      if (!force && p[key] === M.v[key]) return;
      c.querySelectorAll(sel).forEach(fn); p[key] = M.v[key];
    };
    set('clock', '[data-live="clock"]', e => { e.textContent = clockText(); });
    set('pane1', '[data-live="pane1"]', e => { e.innerHTML = pane1HTML(); });
    set('pane2', '[data-live="pane2"]', e => { e.innerHTML = pane2HTML(); });
    set('chat', '[data-live="chat"]', e => { e.innerHTML = chatHTML(); });
    set('linux', '[data-live="linux"]', e => { e.innerHTML = M.linux.slice(-50).map(l => '<span class="ln-row">' + l + '</span>').join(''); });
    set('chart', '[data-live="chart"]', e => { e.setAttribute('d', chartPath()); });
    set('chart', '[data-live="throughput"]', e => { e.textContent = M.throughput.toFixed(1); });
    const smooth = M.video && M.link === 'up' && M.step < 3;
    set('video', '.rd-video', e => {
      e.classList.toggle('smooth', smooth);
      const f = e.querySelector('.frame'); if (f) f.style.transform = smooth ? '' : 'rotate(' + M.videoAngle + 'deg)';
      const b = e.querySelector('[data-live="videobar"]'); if (b) b.style.width = (M.videoPos * 100).toFixed(1) + '%';
      const pl = e.querySelector('[data-live="play"]'); if (pl) pl.textContent = M.video ? '❚❚' : '▶';
    });
    if (h.opts.overlay && (force || p.path !== state().path)) { p.path = state().path; drawZones(h); }
  }
  const bump = k => { M.v[k]++; };

  /* ------------------------------------------------------------------ overlay */
  function drawZones(h) {
    if (!h.opts.overlay) return;
    h.overlay.querySelectorAll('.rs-zone,.rs-zone-label').forEach(n => n.remove());
    const o = h.origin('main'); if (!o) return;
    const st = state();
    const zone = (r, cls, label) => {
      h.overlay.insertAdjacentHTML('beforeend', '<div class="rs-zone ' + cls + '" data-zone="' + label.key + '" style="left:' + (o.x + r.x) + 'px;top:' + (o.y + r.y) + 'px;width:' + r.w + 'px;height:' + r.h + 'px"></div><div class="rs-zone-label ' + cls.split(' ')[0] + '" style="left:' + (o.x + r.x + 8) + 'px;top:' + (o.y + r.y + 8) + 'px">' + label.text + '</div>');
    };
    zone(R.pane1, 'text', { key: 'pane1', text: 'Sent as text' });
    zone(R.pane2, 'text', { key: 'pane2', text: 'Sent as text' });
    zone(R.video, 'video' + (st.path === 'motion' ? ' motion' : ''), { key: 'video', text: st.path === 'motion' ? 'Video · motion stream' : 'Video · tiles, ' + st.ups + '/s' });
  }
  function flash(rectKey, kind) {
    const r = R[rectKey];
    hosts.forEach(h => {
      if (!h.opts.overlay || h.frozen()) return;
      const o = h.origin(r.d); if (!o) return;
      if (kind === 'text' || (kind === 'video' && state().path === 'motion')) {
        const z = h.overlay.querySelector('[data-zone="' + rectKey + '"]');
        if (z) { z.classList.remove('pulse'); void z.offsetWidth; z.classList.add('pulse'); }
        return;
      }
      const x0 = Math.floor((o.x + r.x) / 64), x1 = Math.floor((o.x + r.x + r.w - 1) / 64);
      const y0 = Math.floor((o.y + r.y) / 64), y1 = Math.floor((o.y + r.y + r.h - 1) / 64);
      let html = '';
      for (let ty = y0; ty <= y1; ty++) for (let tx = x0; tx <= x1; tx++) {
        html += '<div class="rs-tile' + (kind === 'cached' ? ' cached' : '') + '" style="left:' + tx * 64 + 'px;top:' + ty * 64 + 'px"></div>';
      }
      h.overlay.insertAdjacentHTML('beforeend', html);
      const tiles = h.overlay.querySelectorAll('.rs-tile');
      if (tiles.length > 400) for (let i = 0; i < tiles.length - 400; i++) tiles[i].remove();
      setTimeout(() => h.overlay.querySelectorAll('.rs-tile').forEach(t => { if (t.getAnimations && !t.getAnimations().length) t.remove(); }), 1000);
    });
  }
  RS.tilesIn = key => { const r = R[key]; return Math.ceil(r.w / 64) * Math.ceil(r.h / 64); };

  /* ------------------------------------------------------------------ interaction */
  function bindInteraction(h) {
    const el = h.el;
    let down = null;
    el.addEventListener('pointerdown', e => {
      if (e.button !== 0 && e.pointerType === 'mouse') return;
      down = { x: e.clientX, y: e.clientY, tx: h.tx, ty: h.ty, cx: h.cursor.x, cy: h.cursor.y, moved: false, target: e.target };
      el.setPointerCapture(e.pointerId);
    });
    el.addEventListener('pointermove', e => {
      const ps = h.pageScale();
      if (!down) {
        if (h.opts.pointer && !h.trackpad && e.pointerType === 'mouse') {
          const r = el.getBoundingClientRect();
          h.cursor.x = ((e.clientX - r.left) / ps - h.tx) / h.s; h.cursor.y = ((e.clientY - r.top) / ps - h.ty) / h.s; h.apply();
        }
        return;
      }
      const dx = (e.clientX - down.x) / ps, dy = (e.clientY - down.y) / ps;
      if (Math.hypot(dx, dy) > 4) down.moved = true;
      if (!down.moved) return;
      if (h.trackpad) {
        const L = LAYOUTS[h.layout];
        h.cursor.x = Math.max(0, Math.min(L.w, down.cx + dx * 1.8 / h.s));
        h.cursor.y = Math.max(0, Math.min(L.h, down.cy + dy * 1.8 / h.s));
        h.apply();
      } else if (h.canPan()) {
        el.classList.add('panning');
        h.tx = down.tx + dx; h.ty = down.ty + dy; h.clamp(); h.apply();
      }
    });
    const end = e => {
      if (!down) return;
      el.classList.remove('panning');
      const d = down; down = null;
      if (d.moved) { if (!h.trackpad) h.emit('panend', { dx: h.tx - d.tx, dy: h.ty - d.ty }); return; }
      if (e.type === 'pointercancel') return;
      const r = el.getBoundingClientRect(), ps = h.pageScale();
      let rx, ry, hit;
      if (h.trackpad) {
        rx = h.cursor.x; ry = h.cursor.y;
        h.pointerEl.style.visibility = 'hidden';
        const cx = r.left + (h.tx + rx * h.s) * ps, cy = r.top + (h.ty + ry * h.s) * ps;
        const under = document.elementFromPoint(cx, cy);
        h.pointerEl.style.visibility = '';
        hit = under && under.closest && under.closest('[data-hit]');
      } else {
        rx = ((e.clientX - r.left) / ps - h.tx) / h.s; ry = ((e.clientY - r.top) / ps - h.ty) / h.s;
        hit = d.target.closest && d.target.closest('[data-hit]');
        if (h.opts.pointer) { h.cursor.x = rx; h.cursor.y = ry; h.apply(); }
      }
      h.emit('click', { rx, ry, hit: hit ? hit.dataset.hit : null });
    };
    el.addEventListener('pointerup', end);
    el.addEventListener('pointercancel', end);
    el.addEventListener('dblclick', e => { e.preventDefault(); h.emit('dblclick', e); });
    el.addEventListener('wheel', e => {
      e.preventDefault();
      if (e.ctrlKey || e.metaKey || h.opts.wheelZoom) h.zoomBy(Math.exp(-e.deltaY * 0.004), e.clientX, e.clientY);
      else if (h.canPan()) { const ps = h.pageScale(); h.tx -= e.deltaX / ps; h.ty -= e.deltaY / ps; h.clamp(); h.apply(); }
    }, { passive: false });
  }

  RS.ripple = (h, rx, ry) => {
    const d = document.createElement('div'); d.className = 'rs-ripple';
    d.style.left = rx + 'px'; d.style.top = ry + 'px'; d.style.transform = 'scale(' + 1 / h.s + ')';
    h.canvas.appendChild(d); setTimeout(() => d.remove(), 650);
  };

  RS.minimap = function (el, target) {
    const mm = RS.mount(el, { layout: target.layout, fit: 'contain', computer: target.opts.computer });
    const vp = document.createElement('div'); vp.className = 'rs-vp'; el.appendChild(vp);
    const update = () => {
      const v = target.view(), L = LAYOUTS[target.layout];
      if (mm.layout !== target.layout) mm.setLayout(target.layout);
      const x = Math.max(0, v.x), y = Math.max(0, v.y), w = Math.min(L.w, v.x + v.w) - x, hh = Math.min(L.h, v.y + v.h) - y;
      vp.style.left = (mm.tx + x * mm.s) + 'px'; vp.style.top = (mm.ty + y * mm.s) + 'px';
      vp.style.width = Math.max(4, w * mm.s) + 'px'; vp.style.height = Math.max(4, hh * mm.s) + 'px';
      el.classList.toggle('whole', target.fit || (w >= L.w - 1 && hh >= L.h - 1));
    };
    target.on('view', update);
    const move = e => {
      const r = el.getBoundingClientRect(), ps = mm.pageScale();
      const rx = ((e.clientX - r.left) / ps - mm.tx) / mm.s, ry = ((e.clientY - r.top) / ps - mm.ty) / mm.s;
      if (target.fit) target.setZoom(Math.max(target.fitScale() * 2.2, 0.5));
      target.centerOn(rx, ry);
    };
    let dragging = false, start = null;
    el.addEventListener('pointerdown', e => { dragging = true; start = target.view(); el.setPointerCapture(e.pointerId); move(e); e.stopPropagation(); });
    el.addEventListener('pointermove', e => { if (dragging) move(e); });
    el.addEventListener('pointerup', () => { if (dragging) { dragging = false; target.emit('panend', { minimap: true, from: start }); } });
    update();
    return mm;
  };

  /* ------------------------------------------------------------------ remote actions */
  RS.remote = function (label, bytes, fn) {
    if (M.link !== 'up') { emit('blocked', label); RS.log(label + ' not sent. mac-studio is not connected.', 'warn'); return false; }
    const rtt = NETS[M.net].rtt;
    RS.log(label + ' · ' + bytes + ' B · reaches mac-studio in about ' + Math.round(rtt / 2) + ' ms', 'input', 'input-' + label, 0);
    setTimeout(fn, rtt / 2);
    return true;
  };
  RS.approve = device => RS.remote('Click on "Yes"', 14, () => {
    if (M.agent !== 'waiting') return;
    M.agent = 'running'; M.device = device || 'this Mac';
    M.agentLines = ['<span class="g">✓ Approved from ' + M.device + '</span>', '<span class="b">●</span> Running ' + REQUESTS[M.request]];
    bump('pane2'); textChange('pane2', 'The agent pane changed', 96); emit('agent', M.agent);
    const results = ['  classify/text       time: [<span class="g">612 µs</span>]', '  classify/photo      time: [<span class="g">1.84 ms</span>]', '  scroll/3-rows       time: [<span class="g">204 µs</span>]  <span class="g">−68%</span>', '<span class="b">●</span> Finished. Scroll detection is 3.1× faster.'];
    results.forEach((line, i) => setTimeout(() => {
      M.agentLines.push(line); bump('pane2'); textChange('pane2', 'Agent printed a line', 58);
      if (i === results.length - 1) { M.agent = 'done'; emit('agent', M.agent); setTimeout(nextRequest, 9000); }
    }, 1400 * (i + 1)));
  });
  RS.deny = device => RS.remote('Click on "No"', 14, () => {
    if (M.agent !== 'waiting') return;
    M.agent = 'denied';
    M.agentLines = ['<span class="r">✗ Denied from ' + (device || 'this Mac') + '</span>', '  Waiting for new instructions.'];
    bump('pane2'); textChange('pane2', 'The agent pane changed', 64); emit('agent', M.agent);
    setTimeout(nextRequest, 6000);
  });
  function nextRequest() {
    M.request = (M.request + 1) % REQUESTS.length; M.agent = 'waiting'; M.agentLines = [];
    bump('pane2'); textChange('pane2', 'Agent asked for approval again', 88); emit('agent', M.agent);
  }
  RS.resetAgent = () => { M.agent = 'waiting'; M.agentLines = []; bump('pane2'); emit('agent', M.agent); };

  RS.typeKey = key => {
    const label = key === 'Enter' ? 'Return key' : key === 'Backspace' ? 'Delete key' : 'Key "' + key + '"';
    return RS.remote(label, 9, () => {
      if (key === 'Backspace') M.typed = M.typed.slice(0, -1);
      else if (key === 'Enter') {
        const cmd = M.typed.trim();
        M.pane1.push('<span class="m">$</span> ' + esc(M.typed));
        if (cmd === 'ls') M.pane1.push('Cargo.toml  crates  design  docs  scripts  tests');
        else if (cmd === 'git status') M.pane1.push('On branch screen-codec', 'nothing to commit, working tree clean');
        else if (cmd === 'date') M.pane1.push('Tue 15 Sep 2026 ' + clockText() + ' CEST');
        else if (cmd === 'clear') M.pane1 = [];
        else if (cmd) M.pane1.push('zsh: command not found: ' + esc(cmd.split(' ')[0]));
        M.typed = '';
      } else if (key.length === 1) M.typed += key;
      bump('pane1'); textChange('pane1', 'Typed character echoed in the terminal', key === 'Enter' ? 80 : 12);
    });
  };
  RS.setVideo = on => {
    if (M.video === on) return;
    RS.remote(on ? 'Click on Play' : 'Click on Pause', 14, () => { M.video = on; bump('video'); emit('video', on); checkPath(); });
  };

  /* ------------------------------------------------------------------ log */
  const lastLog = {};
  const logItems = [];
  RS.log = function (text, kind, key, gap) {
    const now = Date.now();
    if (key && gap && lastLog[key] && now - lastLog[key] < gap) return;
    if (key) lastLog[key] = now;
    logItems.unshift({ t: clockText(), text, kind: kind || 'info' });
    if (logItems.length > 60) logItems.pop();
    document.querySelectorAll('[data-rs-log]').forEach(ul => {
      ul.insertAdjacentHTML('afterbegin', '<li class="rs-log-' + (kind || 'info') + '"><time>' + clockText() + '</time><span>' + text + '</span></li>');
      while (ul.children.length > 60) ul.lastElementChild.remove();
    });
  };
  function textChange(key, what, bytes) {
    flash(key, 'text');
    if (M.link !== 'up') { M.away.text += bytes; return; }
    RS.log(what + ': sent as terminal text, ' + bytes + ' B. As pixels this would be ' + Math.round(RS.tilesIn(key) * 0.25) + ' tiles.', 'text', 'text-' + key, 12000);
  }
  function tileChange(key, what, tiles, kbEach) {
    flash(key, 'tile');
    if (M.link !== 'up') { M.away.tiles += tiles; return; }
    RS.log(what + ': ' + tiles + ' tile' + (tiles === 1 ? '' : 's') + ', ' + (tiles * kbEach).toFixed(1) + ' KB lossless. Everything else on screen was skipped.', 'tile', 'tile-' + key, 15000);
  }

  /* ------------------------------------------------------------------ network control */
  RS.setNet = function (name) {
    if (name === 'offline') {
      if (M.link === 'down') return;
      M.link = 'down'; M.away = { tiles: 0, text: 0 };
      RS.log('Connection lost. The last picture stays on screen; nothing is sent while offline.', 'warn');
      hosts.forEach(h => paint(h)); emit('link', M.link); emit('net', M.net); return;
    }
    const prev = M.net; M.net = name;
    if (M.link !== 'up') {
      M.link = 'reconnecting'; emit('link', M.link);
      RS.log('Reconnecting over ' + NETS[name].route.toLowerCase() + ' (' + NETS[name].medium + ')…', 'warn');
      setTimeout(() => {
        if (M.link !== 'reconnecting') return;
        M.link = 'up';
        const tiles = Math.max(3, M.away.tiles), fromCache = Math.round(tiles * 0.35);
        RS.log('Reconnected in 1.2 s. Caught up by sending only what changed while away: ' + (tiles - fromCache) + ' tiles (' + ((tiles - fromCache) * 0.9).toFixed(1) + ' KB), ' + fromCache + ' from cache, ' + M.away.text + ' B of terminal text. No full refresh.', 'good');
        hosts.forEach(h => paint(h, true)); emit('link', M.link); emit('caughtup', { tiles, fromCache }); checkPath();
      }, 1200);
    } else if (prev !== name) {
      RS.log('Network changed to ' + NETS[name].label + ': ' + NETS[name].rtt + ' ms delay, ' + NETS[name].loss + ' % loss, ' + RS.fmtKbps(NETS[name].capKbps) + ' available.', NETS[name].target >= 3 ? 'warn' : 'info');
    }
    emit('net', M.net); syncControls();
  };
  RS.dropFor = function (ms, reason) {
    const back = M.net;
    RS.log(reason || 'Connection paused.', 'warn');
    RS.setNet('offline');
    setTimeout(() => RS.setNet(back), ms);
  };

  let lastPath = null;
  function checkPath() {
    const st = state();
    if (lastPath && st.path !== lastPath && st.connected) {
      if (st.path === 'motion') RS.log('Video area has changed 30 times a second for 300 ms: now sent as a motion stream (HEVC). The rest of the screen stays pixel-exact.', 'good');
      else if (M.video) RS.log('Connection too slow for a motion stream: the video area falls back to lossy tiles at ' + st.ups + ' updates a second. Text stays sharp.', 'warn');
      else RS.log('Video paused: motion stream ended, and one lossless pass sharpens that area.', 'info');
    }
    lastPath = st.path; hosts.forEach(h => { if (h.opts.overlay) drawZones(h); }); emit('path', st.path);
  }

  /* ------------------------------------------------------------------ main loop */
  let tick = 0;
  function loop() {
    tick++;
    const st0 = state();
    if (tick % 4 === 0) { M.secs++; bump('clock'); if (hosts.some(h => h.layout === 'all' || h.layout === 'studio')) tileChange('clock', 'Clock changed', 2, 0.9); flash('bclock', 'tile'); }
    if (M.video) {
      M.videoPos = (M.videoPos + 0.0025) % 1; M.videoAngle = (M.videoAngle + 24) % 360; bump('video');
      if (st0.path === 'tile' && (st0.ups >= 60 || tick % 2 === 0)) flash('video', 'tile');
      else if (st0.path === 'motion' && tick % 4 === 0) flash('video', 'video');
    }
    if (tick % 14 === 0) {
      const cycle = ['<span class="m">[watch]</span> src/scroll.rs changed', 'test scroll::detects_three_row_offset ... <span class="g">ok</span>', 'test scroll::ignores_cursor_blink ... <span class="g">ok</span>', 'test result: <span class="g">ok</span>. 49 passed; 0 failed'];
      M.pane1.push(cycle[(tick / 14) % cycle.length]); bump('pane1'); textChange('pane1', 'zsh printed a line', 61);
    }
    if (tick % 22 === 0) {
      M.throughput = Math.max(44, Math.min(53, M.throughput + (Math.random() - 0.5) * 2));
      M.chart.shift(); M.chart.push(Math.max(20, Math.min(110, M.chart[M.chart.length - 1] + (Math.random() - 0.45) * 30)));
      bump('chart'); tileChange('chart', 'Chart in the browser redrew', 12, 1.1);
    }
    if (tick % 44 === 0) {
      const msgs = [['Priya', '#E5787A', 'relay-02 latency back to normal'], ['CI', '#74A7F2', 'screen-codec · bench finished'], ['Sam', '#6CCB8F', 'Anyone on the design sync at 16:30?'], ['Lee', '#B69CF0', 'Rolled relay-host back on eu-west']];
      const m = msgs[(tick / 44) % msgs.length];
      M.chat.push({ who: m[0], c: m[1], text: m[2], at: clockText().slice(0, 5) }); bump('chat');
      flash('chat', 'tile');
      if (M.link === 'up') RS.log('New message on the built-in display: 6 tiles changed; 4 matched tiles already on this device (the avatar and bubble), 2 sent, 2.3 KB.', 'tile', 'chat', 20000);
    }
    if (tick % 6 === 0) { M.linux.push(linuxLine(M.linux.length)); bump('linux'); }
    if (tick % 5 === 0 && M.link === 'up') {
      const target = NETS[M.net].target;
      if (M.step !== target) {
        const up = M.step < target;
        M.step += up ? 1 : -1;
        RS.log((up ? 'Keeping up: ' + STEP_ON[M.step] : 'Connection improved: ' + STEP_OFF[M.step + 1]) + '.', up ? 'warn' : 'good');
        emit('step', M.step); checkPath();
      }
    }
    if (tick % 2 === 0) {
      const n = NETS[M.net];
      let demand = 6 + (M.agent === 'running' ? 4 : 0);
      if (M.video) demand += state().path === 'motion' ? (M.step === 0 ? 1400 : 900) : [260, 260, 260, 220, 140, 90][M.step];
      const used = M.link === 'up' ? Math.min(demand, n.capKbps * 0.85) * (0.9 + Math.random() * 0.2) : 0;
      M.samples.push(used); M.samples.shift();
    }
    hosts.forEach(h => paint(h));
    const st = state();
    document.querySelectorAll('[data-stat]').forEach(e => {
      const k = e.dataset.stat; let v = '', warn = false;
      if (!st.connected) v = k === 'route' ? (M.link === 'reconnecting' ? 'Reconnecting' : 'Offline') : '—';
      else if (k === 'rtt') { v = st.rtt + ' ms'; warn = st.rtt > 200; }
      else if (k === 'loss') { v = st.loss.toFixed(1) + ' %'; warn = st.loss > 3; }
      else if (k === 'bandwidth') v = RS.fmtKbps(st.kbps);
      else if (k === 'cap') v = 'of ' + RS.fmtKbps(st.profile.capKbps);
      else if (k === 'route') v = st.route;
      else if (k === 'medium') v = st.medium;
      else if (k === 'ups') v = st.ups + (e.dataset.short ? ' / s' : ' per second');
      else if (k === 'reused') v = st.reused + ' %';
      else if (k === 'pathname') v = st.path === 'motion' ? 'Motion path' : 'Tile path';
      if (e.textContent !== v) e.textContent = v;
      e.classList.toggle('is-warn', warn);
    });
    document.querySelectorAll('[data-rs-spark]').forEach(drawSpark);
    emit('tick', st);
  }
  function drawSpark(svg) {
    const W = 268, H = 38, max = Math.max(50, ...M.samples) * 1.15;
    const pts = M.samples.map((v, i) => [i * W / (M.samples.length - 1), H - 1 - (v / max) * (H - 4)]);
    const line = pts.map((p, i) => (i ? 'L' : 'M') + p[0].toFixed(1) + ' ' + p[1].toFixed(1)).join(' ');
    const warn = M.step >= 3 || M.link !== 'up';
    svg.setAttribute('viewBox', '0 0 ' + W + ' ' + H); svg.setAttribute('preserveAspectRatio', 'none');
    const last = pts[pts.length - 1];
    svg.innerHTML = '<path d="M0 ' + (H - 0.5) + 'H' + W + '" stroke="var(--rs-spark-grid, #2A2E35)" stroke-width="1"/>' +
      '<path d="' + line + ' L' + W + ' ' + H + ' L0 ' + H + 'Z" fill="' + (warn ? 'var(--rs-spark-warn-fill)' : 'var(--rs-spark-fill)') + '"/>' +
      '<path d="' + line + '" fill="none" stroke="' + (warn ? 'var(--rs-spark-warn)' : 'var(--rs-spark)') + '" stroke-width="1.5" vector-effect="non-scaling-stroke"/>' +
      '<circle cx="' + (last[0] - 2) + '" cy="' + last[1].toFixed(1) + '" r="2.5" fill="' + (warn ? 'var(--rs-spark-warn)' : 'var(--rs-spark)') + '"/>';
  }

  RS.ladderHTML = function () {
    const check = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M3 8.5 6.5 12 13 4.5"/></svg>';
    const now = '<svg viewBox="0 0 16 16" fill="currentColor"><circle cx="8" cy="8" r="4.5"/></svg>';
    const next = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="4"/></svg>';
    return STEP_ON.slice(1).map((t, i) => {
      const n = i + 1, cls = n < M.step ? 'done' : n === M.step ? 'now' : 'next';
      return '<li class="rs-step ' + cls + '">' + (cls === 'done' ? check : cls === 'now' ? now : next) + '<span>' + t + '</span></li>';
    }).join('');
  };
  RS.on('step', () => document.querySelectorAll('[data-rs-ladder]').forEach(ul => { ul.innerHTML = RS.ladderHTML(); }));

  /* ------------------------------------------------------------------ page controls + tour */
  function syncControls() {
    document.querySelectorAll('[data-rs-net]').forEach(b => b.setAttribute('aria-pressed', String(M.link === 'down' ? b.dataset.rsNet === 'offline' : b.dataset.rsNet === M.net)));
    document.querySelectorAll('[data-rs-video]').forEach(b => b.setAttribute('aria-pressed', String(M.video)));
  }
  RS.on('link', syncControls); RS.on('video', syncControls);

  RS.tour = function (items) {
    const done = new Set();
    const render = () => document.querySelectorAll('[data-rs-tour]').forEach(ol => {
      ol.innerHTML = items.map(it => '<li class="' + (done.has(it.id) ? 'done' : '') + '"><span class="tick" aria-hidden="true"></span><span>' + it.text + '</span></li>').join('');
      const c = document.querySelector('[data-rs-tour-count]'); if (c) c.textContent = done.size + ' of ' + items.length;
    });
    RS.done = id => { if (!done.has(id) && items.some(i => i.id === id)) { done.add(id); render(); } };
    render();
  };
  RS.done = () => {};

  document.addEventListener('click', e => {
    const b = e.target.closest('[data-rs-net],[data-rs-video],[data-rs-agent]');
    if (!b) return;
    if (b.dataset.rsNet) { RS.setNet(b.dataset.rsNet); if (b.dataset.rsNet !== 'good') RS.done('network'); if (b.dataset.rsNet === 'offline') RS.done('offline'); }
    if (b.hasAttribute('data-rs-video')) RS.setVideo(!M.video);
    if (b.hasAttribute('data-rs-agent')) nextRequest();
  });

  RS.start = function () {
    syncControls();
    document.querySelectorAll('[data-rs-ladder]').forEach(ul => { ul.innerHTML = RS.ladderHTML(); });
    lastPath = state().path;
    RS.log('mac-studio connected directly over Wi-Fi. Only changed parts of the screen are sent; unchanged pixels cost nothing.', 'good');
    loop();
    setInterval(loop, 250);
  };
})();
