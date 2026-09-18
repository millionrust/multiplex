/* Phone prototype controller shared by ios.html and android.html. Markup differs per platform;
 * behaviour is wired through data attributes so both pages behave identically. */
(function () {
  'use strict';
  const RS = window.RS;
  const $ = (s, r) => (r || document).querySelector(s);
  const $$ = (s, r) => Array.from((r || document).querySelectorAll(s));

  window.RSPhone = {
    init(cfg) {
      const phone = $('[data-phone-root]');
      const device = cfg.device;
      let screen = 'fleet', history = [], controlling = false, keyboard = false, trackpad = false, weakDismissed = false;

      /* previews */
      $$('[data-preview]', phone).forEach(el => RS.mount(el, { layout: el.dataset.preview === 'linux' ? 'linux' : 'studio', fit: 'cover' }));

      /* viewer stage */
      const stageEl = $('[data-rs-stage]', phone);
      const stage = RS.mount(stageEl, { layout: 'studio', fit: 'interactive', receiver: true, wheelZoom: true, zoom: 1.22, center: [1335, 330] });
      const mini = $('[data-mini]', phone);
      if (mini) RS.minimap(mini, stage);

      const toast = (text, ms) => {
        const t = $('[data-toast]', phone); if (!t) return;
        t.querySelector('[data-toast-text]').textContent = text; t.hidden = false;
        clearTimeout(t._h); t._h = setTimeout(() => { t.hidden = true; }, ms || 2600);
      };

      function go(name, push) {
        if (name === screen) return;
        if (push !== false) history.push(screen);
        screen = name;
        $$('[data-screen]', phone).forEach(s => { s.hidden = s.dataset.screen !== name; });
        phone.dataset.current = name;
        closeSheets();
        if (name === 'viewer') {
          RS.done('open');
          requestAnimationFrame(() => {
            if (!stage.shown) { stage.shown = true; stage.fit = false; stage.s = 1.22; stage.centerOn(1335, 330); }
            stage.relayout(); updateZoomChip();
          });
        }
        if (name !== 'viewer' && phone.classList.contains('land')) rotate(false);
        updateChrome();
      }
      function back() { const prev = history.pop() || 'fleet'; go(prev, false); }

      function closeSheets() { $$('[data-sheet]', phone).forEach(s => { s.hidden = true; }); const sc = $('[data-scrim]', phone); if (sc) sc.hidden = true; }
      function openSheet(name) {
        closeSheets();
        const s = $('[data-sheet="' + name + '"]', phone); if (!s) return;
        s.hidden = false; const sc = $('[data-scrim]', phone); if (sc) sc.hidden = false;
        if (name === 'conn') { $$('[data-rs-ladder]', s).forEach(ul => { ul.innerHTML = RS.ladderHTML(); }); if (RS.model.net !== 'good') RS.done('details'); }
      }

      function setControl(on) {
        controlling = on;
        stage.setPointer(false);
        $$('[data-act="control"]', phone).forEach(b => { b.classList.toggle('watch', !on); b.setAttribute('aria-pressed', String(on)); });
        $$('[data-control-label]', phone).forEach(e => { e.textContent = on ? 'Controlling' : 'Take control'; });
        RS.log(on ? device + ' took control of mac-studio. Other viewers now watch.' : device + ' gave control back and is watching.', 'input');
        if (on) RS.done('control');
      }
      function setKeyboard(on) {
        keyboard = on; phone.classList.toggle('kbd-open', on);
        $$('[data-act="keyboard"]', phone).forEach(b => b.classList.toggle('on', on));
        requestAnimationFrame(() => stage.relayout());
      }
      function setTrackpad(on) {
        trackpad = on; stage.setTrackpad(on);
        $$('[data-act="pointer"]', phone).forEach(b => b.classList.toggle('on', !on));
        $$('[data-pointer-label]', phone).forEach(e => { e.textContent = on ? 'Trackpad' : 'Touch'; });
        toast(on ? 'Trackpad: drag moves the pointer, tap clicks where it points' : 'Touch: tap clicks where your finger is', 2400);
        if (on) RS.done('trackpad');
      }
      function rotate(on) {
        phone.classList.toggle('land', on);
        window.dispatchEvent(new Event('rs-refit'));
        requestAnimationFrame(() => stage.relayout());
      }

      function needControl() {
        if (controlling) return true;
        toast('Watching only. Tap Take control to click or type on mac-studio.');
        return false;
      }

      stage.on('click', ({ rx, ry, hit }) => {
        if (RS.model.link !== 'up') { toast('Not connected. Clicks are not queued.'); return; }
        if (!needControl()) return;
        RS.ripple(stage, rx, ry);
        if (hit === 'approve') { RS.approve(device); RS.done('approve'); }
        else if (hit === 'deny') RS.deny(device);
        else if (hit === 'video') RS.setVideo(!RS.model.video);
        else RS.remote('Click at ' + Math.round(rx) + ', ' + Math.round(ry), 14, () => {});
      });
      stage.on('dblclick', e => { stage.fit ? stage.setZoom(1.22, e.clientX, e.clientY) : stage.setZoom('fit'); RS.done('zoom'); });
      stage.on('zoom', () => { RS.done('zoom'); updateZoomChip(); });
      stage.on('panend', d => {
        RS.done('zoom');
        if (RS.model.step >= 2 && RS.model.link === 'up') {
          const moved = Math.abs(d.dx || 0) + Math.abs(d.dy || 0);
          const n = Math.max(4, Math.min(60, Math.round(moved / stage.s / 64 * 6)));
          const cached = Math.round(n * 0.7);
          RS.log('Brought ' + n + ' paused tiles into view: ' + cached + ' came from this phone\'s cache (0 B), ' + (n - cached) + ' were sent (' + ((n - cached) * 1.1).toFixed(1) + ' KB).', 'tile', 'pan', 2500);
        }
      });
      function updateZoomChip() {
        $$('[data-zoom-label]', phone).forEach(e => { e.textContent = stage.fit ? 'Fit' : (stage.s / stage.fitScale()).toFixed(1) + '×'; });
      }

      /* actions */
      phone.addEventListener('click', e => {
        const t = e.target.closest('[data-go],[data-back],[data-act],[data-close-sheet],[data-display-choice],[data-key],[data-approve],[data-deny]');
        if (!t || !phone.contains(t)) return;
        if (t.dataset.go) go(t.dataset.go);
        else if (t.hasAttribute('data-back')) back();
        else if (t.hasAttribute('data-close-sheet')) closeSheets();
        else if (t.dataset.displayChoice) {
          stage.setLayout(t.dataset.displayChoice); stage.setZoom('fit');
          $$('[data-display-choice]', phone).forEach(b => b.setAttribute('aria-pressed', String(b === t)));
          RS.log('Switched to ' + t.textContent.trim().split('\n')[0] + '. Tiles already on this phone were reused; nothing is recaptured.', 'info');
          closeSheets();
        } else if (t.dataset.key) {
          if (!needControl()) return;
          const k = t.dataset.key;
          if (RS.typeKey(k) && k === 'Enter') RS.done('type');
        } else if (t.hasAttribute('data-approve')) { if (needControl()) { RS.approve(device); RS.done('approve'); } }
        else if (t.hasAttribute('data-deny')) { if (needControl()) RS.deny(device); }
        else {
          const a = t.dataset.act;
          if (a === 'control') setControl(!controlling);
          else if (a === 'keyboard') { if (!keyboard && !controlling) { needControl(); return; } setKeyboard(!keyboard); }
          else if (a === 'pointer') setTrackpad(!trackpad);
          else if (a === 'displays') openSheet('displays');
          else if (a === 'conn') openSheet('conn');
          else if (a === 'dismiss-weak') { weakDismissed = true; updateChrome(); }
          else if (a === 'terminals-only') { go('detail'); }
        }
      });
      const scrim = $('[data-scrim]', phone); if (scrim) scrim.addEventListener('click', closeSheets);

      document.addEventListener('keydown', e => {
        if (screen !== 'viewer' || e.metaKey || e.ctrlKey || e.altKey) return;
        if (e.target.closest('input,textarea,select')) return;
        const k = e.key === 'Enter' || e.key === 'Backspace' ? e.key : e.key.length === 1 ? e.key : null;
        if (!k) return;
        e.preventDefault();
        if (!needControl()) return;
        if (RS.typeKey(k) && k === 'Enter') RS.done('type');
      });

      /* page-level controls */
      document.addEventListener('click', e => {
        const b = e.target.closest('[data-phone],[data-jump]');
        if (!b) return;
        const p = b.dataset.phone, j = b.dataset.jump;
        if (p === 'rotate') { if (screen !== 'viewer') go('viewer'); rotate(!phone.classList.contains('land')); }
        if (p === 'lock') lock();
        if (p === 'zoomin') { if (screen !== 'viewer') go('viewer'); stage.zoomBy(1.4); }
        if (p === 'zoomout') { if (screen !== 'viewer') go('viewer'); stage.zoomBy(1 / 1.4); }
        if (j) {
          if (j === 'fleet' || j === 'detail') { history = []; go(j, false); }
          if (j === 'viewer') go('viewer');
          if (j === 'weak') { go('viewer'); RS.setNet('weak'); RS.done('network'); setTimeout(() => openSheet('conn'), 200); }
          if (j === 'typing') { go('viewer'); if (!controlling) setControl(true); rotate(true); setKeyboard(true); }
        }
      });

      function lock() {
        const l = $('[data-lock]', phone);
        l.hidden = false;
        RS.dropFor(4200, device + ' locked. TermiRust keeps the last picture and closes the connection to save battery.');
        setTimeout(() => { l.hidden = true; RS.done('lock'); }, 3000);
      }

      function updateChrome() {
        const st = RS.state();
        $$('[data-conn-chip]', phone).forEach(c => {
          c.classList.toggle('warn', st.connected && st.step >= 3);
          c.classList.toggle('off', !st.connected);
        });
        $$('[data-conn-text]', phone).forEach(e => {
          e.textContent = st.link === 'down' ? 'Offline' : st.link === 'reconnecting' ? 'Reconnecting…' : st.route + ' · ' + st.rtt + ' ms';
        });
        const weak = st.connected && st.step >= 3;
        if (!weak) weakDismissed = false;
        $$('[data-weak-banner]', phone).forEach(b => { b.hidden = !weak || weakDismissed; });
        $$('[data-reconnect]', phone).forEach(r => { r.hidden = st.connected; });
        $$('[data-reconnect-text]', phone).forEach(e => {
          e.textContent = st.link === 'reconnecting' ? 'Reconnecting to mac-studio…' : 'Offline. Showing the screen from ' + RS.clock().slice(0, 5) + '.';
        });
        const a = RS.model.agent;
        $$('[data-agent-text]', phone).forEach(e => {
          e.textContent = a === 'waiting' ? '1 agent waiting for approval' : a === 'running' ? 'Agent is running a benchmark' : 'No agents waiting';
          e.classList.toggle('quiet', a !== 'waiting');
        });
        $$('[data-agent-sub]', phone).forEach(e => {
          e.textContent = a === 'waiting' ? 'Waiting for approval' : a === 'running' ? 'Running cargo bench' : a === 'denied' ? 'Denied · waiting for instructions' : 'Finished · 3.1× faster';
          e.classList.toggle('quiet', a !== 'waiting');
        });
        $$('[data-agent-live]', phone).forEach(e => { e.hidden = a !== 'waiting'; });
        updateZoomChip();
      }
      RS.on('tick', updateChrome);
      RS.on('link', updateChrome);
      RS.on('agent', updateChrome);
      RS.on('caughtup', ({ tiles, fromCache }) => { if (screen === 'viewer') toast('Caught up · ' + (tiles - fromCache) + ' tiles sent, ' + fromCache + ' from cache', 3200); });

      /* terminal screen mirrors the agent pane as text */
      const term = $('[data-term-live]', phone);
      if (term) {
        const draw = () => {
          const tmp = document.createElement('div');
          tmp.innerHTML = '<div class="rd"><div data-live="pane2"></div></div>';
          const host = RS.hosts[0];
          const src = host && host.canvas.querySelector('[data-live="pane2"]');
          term.innerHTML = src ? src.innerHTML : '';
        };
        RS.on('tick', draw);
      }

      go('fleet', false);
      phone.dataset.current = 'fleet';
      $$('[data-screen]', phone).forEach(s => { s.hidden = s.dataset.screen !== 'fleet'; });
      setTimeout(() => { $$('[data-act="pointer"]', phone).forEach(b => b.classList.add('on')); }, 0);
      updateChrome();
    }
  };
})();
