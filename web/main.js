let currentSessionId = null;
const messagesEl = document.getElementById('messages');
const sessionListEl = document.getElementById('session-list');
const inputEl = document.getElementById('composer-input');
const formEl = document.getElementById('composer-form');
const sendBtn = document.getElementById('send-btn');
const newChatBtn = document.getElementById('new-chat-btn');

async function loadSessions() {
  try {
    const res = await fetch('/api/sessions');
    if (!res.ok) return;
    const sessions = await res.json();
    sessionListEl.innerHTML = '';
    sessions.forEach(s => {
      const li = document.createElement('li');
      li.className = 'session-item' + (s.id === currentSessionId ? ' active' : '');
      const title = document.createElement('span');
      title.className = 'session-title';
      title.textContent = s.first_user_text || (s.id ? s.id.slice(0, 8) : 'Session');
      title.onclick = () => selectSession(s.id);
      const delBtn = document.createElement('button');
      delBtn.className = 'session-del';
      delBtn.textContent = '×';
      delBtn.onclick = (e) => { e.stopPropagation(); deleteSession(s.id); };
      li.appendChild(title);
      li.appendChild(delBtn);
      sessionListEl.appendChild(li);
    });
  } catch (err) {
    console.error('Failed to load sessions:', err);
  }
}

async function selectSession(id) {
  currentSessionId = id;
  messagesEl.innerHTML = '';
  loadSessions();
  try {
    const res = await fetch(`/api/sessions/${id}`);
    if (!res.ok) return;
    const entries = await res.json();
    entries.forEach(entry => {
      const msg = entry.message || entry;
      if (msg.role === 'user') {
        const text = (msg.content || []).map(b => b.text || (typeof b === 'string' ? b : '')).join('');
        appendUserMessage(text);
      } else if (msg.role === 'assistant') {
        const container = createAssistantContainer();
        (msg.content || []).forEach(block => {
          if (block.type === 'Text' || block.text) {
            appendAssistantText(container, block.text || block.type);
          } else if (block.type === 'ToolUse' || block.name) {
            appendToolChip(container, block.name);
          } else if (block.type === 'ToolResult') {
            if (block.is_error) appendError(container, block.content);
          }
        });
      }
    });
    scrollToBottom();
  } catch (err) {
    console.error('Failed to load session history:', err);
  }
}

async function deleteSession(id) {
  try {
    await fetch(`/api/sessions/${id}`, { method: 'DELETE' });
    if (currentSessionId === id) {
      currentSessionId = null;
      messagesEl.innerHTML = '';
    }
    loadSessions();
  } catch (err) {
    console.error('Failed to delete session:', err);
  }
}

function scrollToBottom() {
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

function appendUserMessage(text) {
  const el = document.createElement('div');
  el.className = 'message user';
  el.innerHTML = '<div class="sender">User</div><div class="msg-content"></div>';
  el.querySelector('.msg-content').textContent = text;
  messagesEl.appendChild(el);
  scrollToBottom();
}

function createAssistantContainer() {
  const el = document.createElement('div');
  el.className = 'message assistant';
  el.innerHTML = '<div class="sender">gray</div><div class="msg-content"></div>';
  messagesEl.appendChild(el);
  scrollToBottom();
  return el.querySelector('.msg-content');
}

function appendAssistantText(container, delta) {
  container.appendChild(document.createTextNode(delta));
  scrollToBottom();
}

function appendToolChip(container, name) {
  const chip = document.createElement('span');
  chip.className = 'tool-chip';
  chip.textContent = `[${name}]`;
  container.appendChild(chip);
  scrollToBottom();
}

function appendError(container, text) {
  const errEl = document.createElement('div');
  errEl.className = 'error-text';
  errEl.textContent = `! ${text}`;
  container.appendChild(errEl);
  scrollToBottom();
}

async function sendMessage(text) {
  if (!text.trim()) return;
  appendUserMessage(text);
  inputEl.value = '';
  inputEl.disabled = true;
  sendBtn.disabled = true;

  const assistantContainer = createAssistantContainer();

  try {
    const res = await fetch('/api/chat', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ session_id: currentSessionId, message: text })
    });

    const headerSessionId = res.headers.get('x-session-id');
    if (headerSessionId) currentSessionId = headerSessionId;

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop();

      for (const line of lines) {
        if (line.startsWith('data:')) {
          const dataStr = line.slice(5).trim();
          if (!dataStr || dataStr === '[DONE]') continue;
          try {
            const ev = JSON.parse(dataStr);
            if (ev.type === 'TextDelta') {
              appendAssistantText(assistantContainer, ev.delta);
            } else if (ev.type === 'ToolCallStart') {
              appendToolChip(assistantContainer, ev.name);
            } else if (ev.type === 'ToolResult') {
              if (ev.is_error) appendError(assistantContainer, ev.output);
            }
          } catch (e) {
            console.error('Failed to parse SSE JSON:', e, dataStr);
          }
        }
      }
    }
  } catch (err) {
    appendError(assistantContainer, err.message || 'Stream error');
  } finally {
    inputEl.disabled = false;
    sendBtn.disabled = false;
    inputEl.focus();
    loadSessions();
  }
}

formEl.onsubmit = (e) => {
  e.preventDefault();
  sendMessage(inputEl.value);
};

inputEl.onkeydown = (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    sendMessage(inputEl.value);
  }
};

newChatBtn.onclick = () => {
  currentSessionId = null;
  messagesEl.innerHTML = '';
  loadSessions();
  inputEl.focus();
};

loadSessions();
