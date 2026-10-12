const test = require('node:test');
const assert = require('node:assert/strict');
const { parseTerminal, projectEvents } = require('./mobile-agent-chat-model.js');

test('colored prompts become readable roles, while code retains speaker-like lines', () => {
  const model = parseTerminal('\x1b[32m❯ Explain this\x1b[0m\nClaude: Here is the code.\n```text\nUser: this is code\n› also code\n```\n❯ Thanks');
  assert.deepEqual(model.items.map(item => item.role), ['user', 'assistant', 'user']);
  assert.match(model.items[1].text, /User: this is code/);
  assert.match(model.items[1].text, /› also code/);
});
test('unknown terminal output is preserved and never becomes an approval', () => {
  const model = parseTerminal('Allow this command?\nrm -rf example\n[y/N]');
  assert.equal(model.items[0].role, 'unknown');
  assert.equal(model.items[0].text, 'Allow this command?\nrm -rf example\n[y/N]');
  assert.equal(model.items.some(item => item.type === 'approval'), false);
  assert.equal(model.state, 'view-only');
});
test('structured deltas combine but tools keep message boundaries', () => {
  const model = projectEvents([{type:'message_delta',role:'assistant',text:'Hello '},{type:'message_delta',role:'assistant',text:'world'},{type:'tool_started',call_id:'1',name:'Read file'},{type:'tool_finished',call_id:'1',outcome:'succeeded'},{type:'message_delta',role:'assistant',text:'Done'}]);
  assert.equal(model.items[0].text, 'Hello world');
  assert.equal(model.items[1].state, 'succeeded');
  assert.equal(model.items[2].text, 'Done');
});
test('an approval resolution survives subsequent messages', () => {
  const events = [{type:'approval_requested',request_id:'a1',operation:'Run tests'},{type:'approval_resolved',request_id:'a1',approved:true},{type:'state_changed',state:'ready'},{type:'message_delta',role:'user',text:'Continue'}];
  const model = projectEvents(events);
  assert.equal(model.items[0].state, 'approved');
  assert.equal(model.items[1].text, 'Continue');
  assert.equal(model.state, 'ready');
});
test('redraws and truncation are disclosed, rather than treated as complete history', () => {
  const model = parseTerminal('x'.repeat(300000)+'\x1b[2J\nClaude: A new screen');
  assert.ok(model.warnings.some(w => w.includes('omitted')));
  assert.ok(model.warnings.some(w => w.includes('redraws')));
  assert.ok(model.items.every(item => item.text.length <= 256*1024));
});
test('Unicode and hyperlink text remain text, without terminal control strings', () => {
  const model = parseTerminal('Assistant: Héllo 👋 \x1b]8;;https://example.com\x07a link\x1b]8;;\x07');
  assert.equal(model.items[0].text, 'Héllo 👋 a link');
});
