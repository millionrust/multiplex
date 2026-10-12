/* Prototype projection only. Terminal text is never an approval/control source. */
const AgentChatModel = (() => {
  const MAX_TEXT = 256 * 1024;
  const MAX_ITEMS = 1000;

  function parseTerminal(source) {
    const warnings = [];
    if (source.length > MAX_TEXT) { source = source.slice(-MAX_TEXT); warnings.push('Earlier output was omitted.'); }
    if (/\x1b\[[0-?]*[ -/]*[ABCDHJKf]/.test(source)) warnings.push('Screen redraws may hide parts of the conversation.');
    const clean = source.replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, '').replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '');
    const lines = []; let row = [], column = 0;
    for (const char of clean) {
      if (char === '\r') { column = 0; continue; }
      if (char === '\n') { lines.push(row.join('')); row = []; column = 0; continue; }
      if (char === '\b') { column = Math.max(0, column - 1); continue; }
      if (char !== '\t' && char.codePointAt(0) < 32) continue;
      row[column++] = char;
    }
    if (row.length) lines.push(row.join(''));
    const items = []; let current = null, fenced = false;
    function begin(role, text) {
      current = { id: `parsed-${items.length}`, type: 'message', role, text, source: 'parsed' };
      items.push(current);
    }
    for (const line of lines) {
      if (/^\s*```/.test(line)) {
        if (current) current.text += `\n${line}`; else begin('unknown', line);
        fenced = !fenced; continue;
      }
      if (fenced) { current.text += `\n${line}`; continue; }
      let match;
      if ((match = line.match(/^\s*(?:User:|You:|[›❯])\s*(.*)$/u))) begin('user', match[1]);
      else if ((match = line.match(/^\s*(?:Assistant:|Claude:|Codex:)\s*(.*)$/u))) begin('assistant', match[1]);
      else if (current) current.text += `\n${line}`;
      else if (line.trim()) begin('unknown', line);
    }
    for (const item of items) item.text = item.text.trim();
    if (items.some(item => item.role === 'unknown') || !items.some(item => item.role === 'assistant')) warnings.push('Some output could not be assigned to a speaker.');
    if (items.length > MAX_ITEMS) warnings.push('Earlier messages were omitted.');
    return { items: items.slice(-MAX_ITEMS), warnings, source: 'parsed', state: 'view-only' };
  }

  function projectEvents(events) {
    const items = []; const tools = new Map(), approvals = new Map(); let current = null; let state = 'ready';
    for (const event of events.slice(-MAX_ITEMS * 4)) {
      if (!event || typeof event !== 'object') continue;
      if (event.type === 'message_delta') {
        if (!['user', 'assistant'].includes(event.role) || typeof event.text !== 'string') continue;
        if (!current || current.role !== event.role) {
          current = { id: `message-${items.length}`, type: 'message', role: event.role, text: '', source: 'structured' };
          items.push(current);
        }
        current.text = (current.text + event.text).slice(0, MAX_TEXT);
      } else if (event.type === 'tool_started') {
        current = null;
        const tool = { id: event.call_id, type: 'tool', title: event.name, text: event.summary || '', state: 'running' };
        tools.set(event.call_id, tool); items.push(tool);
      } else if (event.type === 'tool_finished') {
        const tool = tools.get(event.call_id);
        if (tool) { tool.state = event.outcome; tool.text = (tool.text + (event.output ? `\n\n${event.output}` : '')).slice(0, MAX_TEXT); }
      } else if (event.type === 'approval_requested' && event.request_id) {
        current = null; state = 'needs-approval';
        const approval = { id: event.request_id, type: 'approval', title: event.operation, text: event.reason || '', state: 'pending' };
        approvals.set(event.request_id, approval); items.push(approval);
      } else if (event.type === 'approval_resolved') {
        const approval = approvals.get(event.request_id);
        if (approval && approval.state === 'pending') approval.state = event.approved ? 'approved' : 'declined';
      } else if (event.type === 'state_changed') {
        state = event.state;
        if (['ready', 'running', 'completed', 'disconnected'].includes(state)) current = null;
      } else if (event.type === 'diagnostic' && event.message) {
        current = null; items.push({ id: `output-${items.length}`, type: 'message', role: 'unknown', text: String(event.message).slice(0, MAX_TEXT) });
      }
    }
    return { items: items.slice(-MAX_ITEMS), warnings: [], source: 'structured', state };
  }
  return { parseTerminal, projectEvents };
})();
if (typeof module !== 'undefined') module.exports = AgentChatModel;
