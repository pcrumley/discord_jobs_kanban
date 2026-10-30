// Job Kanban — frontend. The board is a plain object; every mutation calls
// render() and schedules a save through the Tauri backend.

const tauri = window.__TAURI__;

// Storage adapter: Tauri commands when running in the app, localStorage when
// opened in a plain browser (handy for quick UI tweaks).
const store = tauri
  ? {
      load: () => tauri.core.invoke('load_board'),
      save: (board) => tauri.core.invoke('save_board', { board }),
      path: () => tauri.core.invoke('board_path'),
      openUrl: (url) => tauri.opener.openUrl(url),
      takeInbox: () => tauri.core.invoke('take_inbox'),
      onInbox: (fn) => tauri.event.listen('inbox-changed', fn),
      fetchJob: (url) => tauri.core.invoke('fetch_job_details', { url }),
      getSettings: () => tauri.core.invoke('get_settings'),
      saveSettings: (settings) => tauri.core.invoke('save_settings', { settings }),
      testDiscord: (token, channelId) => tauri.core.invoke('test_discord', { token, channelId }),
      discordStatus: () => tauri.core.invoke('discord_status'),
    }
  : {
      load: async () => JSON.parse(localStorage.getItem('board') || 'null') || defaultBoard(),
      save: async (board) => localStorage.setItem('board', JSON.stringify(board)),
      path: async () => 'browser localStorage',
      openUrl: async (url) => window.open(url, '_blank'),
      takeInbox: async () => [],
      onInbox: async () => {},
      fetchJob: async () => { throw new Error('only available in the desktop app'); },
      getSettings: async () => ({}),
      saveSettings: async () => { throw new Error('only available in the desktop app'); },
      testDiscord: async () => { throw new Error('only available in the desktop app'); },
      discordStatus: async () => ({ configured: false }),
    };

function defaultBoard() {
  const col = (id, title) => ({ id, title, cards: [] });
  return {
    columns: [
      col('inbox', 'Inbox'),
      col('wishlist', 'Wishlist'),
      col('applied', 'Applied'),
      col('interviewing', 'Interviewing'),
      col('offer', 'Offer'),
      col('rejected', 'Rejected'),
    ],
  };
}

let board = defaultBoard();
let filter = '';

/// Boards saved before the Inbox column existed get it added at the front.
function ensureInboxColumn(b) {
  if (!b.columns.some((c) => c.id === 'inbox')) {
    b.columns.unshift({ id: 'inbox', title: 'Inbox', cards: [] });
  }
  return b;
}

const column = (id) => board.columns.find((c) => c.id === id);
const inboxCards = () => (column('inbox')?.cards) || [];

const $ = (sel) => document.querySelector(sel);
const boardEl = $('#board');
const statusEl = $('#status');
const dialog = $('#card-dialog');
const form = $('#card-form');

// ---------- persistence ----------

let saveTimer = null;
function scheduleSave() {
  setStatus('Saving…');
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      await store.save(board);
      setStatus('Saved');
    } catch (e) {
      setStatus('Save failed: ' + e, true);
    }
  }, 250);
}

function setStatus(text, isError = false) {
  statusEl.textContent = text;
  statusEl.classList.toggle('error', isError);
}

// ---------- model helpers ----------

const uid = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
const now = () => new Date().toISOString();

function findCard(id) {
  for (const col of board.columns) {
    const idx = col.cards.findIndex((c) => c.id === id);
    if (idx !== -1) return { col, idx, card: col.cards[idx] };
  }
  return null;
}

function moveCard(cardId, toColId, toIndex) {
  const from = findCard(cardId);
  if (!from) return;
  const toCol = board.columns.find((c) => c.id === toColId);
  if (!toCol) return;

  from.col.cards.splice(from.idx, 1);
  if (from.col === toCol && from.idx < toIndex) toIndex--;
  toIndex = Math.max(0, Math.min(toIndex, toCol.cards.length));
  if (from.col !== toCol) from.card.stage_changed_at = now();
  toCol.cards.splice(toIndex, 0, from.card);

  render();
  scheduleSave();
}

// ---------- rendering ----------

function matches(card) {
  if (!filter) return true;
  const hay = [card.company, card.role, card.location, card.notes, card.contact].join(' ').toLowerCase();
  return hay.includes(filter);
}

function daysSince(iso) {
  if (!iso) return null;
  return Math.floor((Date.now() - new Date(iso).getTime()) / 86400000);
}

function render() {
  const n = inboxCards().length;
  $('#triage-btn').hidden = n === 0;
  $('#triage-count').textContent = `(${n})`;
  boardEl.textContent = '';
  for (const col of board.columns) {
    const colEl = document.createElement('section');
    colEl.className = 'column';
    colEl.dataset.id = col.id;

    const visible = col.cards.filter(matches);

    const header = document.createElement('div');
    header.className = 'column-header';
    header.innerHTML = `<span>${esc(col.title)}</span><span class="count">${visible.length}</span>`;
    const addBtn = document.createElement('button');
    addBtn.className = 'add';
    addBtn.title = 'Add card to ' + col.title;
    addBtn.textContent = '+';
    addBtn.addEventListener('click', () => openDialog(null, col.id));
    header.appendChild(addBtn);
    colEl.appendChild(header);

    const cardsEl = document.createElement('div');
    cardsEl.className = 'cards';
    cardsEl.dataset.col = col.id;
    attachDropZone(cardsEl);

    if (visible.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'empty';
      empty.textContent = filter ? 'No matches' : 'Drop a card here';
      cardsEl.appendChild(empty);
    }
    for (const card of visible) cardsEl.appendChild(renderCard(card, col));

    colEl.appendChild(cardsEl);
    boardEl.appendChild(colEl);
  }
}

function renderCard(card, col) {
  const el = document.createElement('article');
  el.className = 'card' + (card.unreviewed ? ' unreviewed' : '');
  el.draggable = true;
  el.dataset.id = card.id;

  const stageDays = daysSince(card.stage_changed_at);
  const isStale = stageDays !== null && stageDays >= 14 && !['inbox', 'offer', 'rejected'].includes(col.id);
  const bits = [];
  if (card.location) bits.push(esc(card.location));
  if (card.salary) bits.push(esc(card.salary));

  el.innerHTML = `
    <div class="company">${esc(card.company) || '<em>Untitled</em>'}${card.unreviewed ? ' <span class="badge">new</span>' : ''}</div>
    ${card.role ? `<div class="role">${esc(card.role)}</div>` : ''}
    <div class="line">
      ${bits.length ? `<span>${bits.join(' · ')}</span>` : ''}
      <span class="spacer"></span>
      ${stageDays !== null ? `<span class="${isStale ? 'stale' : ''}" title="Days in this stage">${stageDays}d</span>` : ''}
      ${card.url ? `<a class="link" href="#" title="${esc(card.url)}">link ↗</a>` : ''}
    </div>`;

  const link = el.querySelector('.link');
  if (link) {
    link.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      store.openUrl(card.url).catch((err) => setStatus('Could not open link: ' + err, true));
    });
  }

  el.addEventListener('click', () => openDialog(card.id));
  el.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData('text/plain', card.id);
    e.dataTransfer.effectAllowed = 'move';
    requestAnimationFrame(() => el.classList.add('dragging'));
  });
  el.addEventListener('dragend', () => {
    el.classList.remove('dragging');
    clearPlaceholders();
  });
  return el;
}

function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

// ---------- drag and drop ----------

function clearPlaceholders() {
  document.querySelectorAll('.placeholder').forEach((p) => p.remove());
  document.querySelectorAll('.cards.drag-over').forEach((z) => z.classList.remove('drag-over'));
}

function attachDropZone(zone) {
  zone.addEventListener('dragover', (e) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    zone.classList.add('drag-over');

    // Place the placeholder before the first non-dragged card whose midpoint is below the cursor.
    let placeholder = zone.querySelector('.placeholder');
    if (!placeholder) {
      clearPlaceholders();
      zone.classList.add('drag-over');
      placeholder = document.createElement('div');
      placeholder.className = 'placeholder';
    }
    const cards = [...zone.querySelectorAll('.card:not(.dragging)')];
    const next = cards.find((c) => {
      const r = c.getBoundingClientRect();
      return e.clientY < r.top + r.height / 2;
    });
    if (next) zone.insertBefore(placeholder, next);
    else zone.appendChild(placeholder);
  });

  zone.addEventListener('dragleave', (e) => {
    if (!zone.contains(e.relatedTarget)) {
      zone.classList.remove('drag-over');
      zone.querySelector('.placeholder')?.remove();
    }
  });

  zone.addEventListener('drop', (e) => {
    e.preventDefault();
    const cardId = e.dataTransfer.getData('text/plain');
    const placeholder = zone.querySelector('.placeholder');
    // Index among visible, non-dragged cards; map that to a model index.
    const siblings = [...zone.children].filter((c) => c.classList.contains('card') && !c.classList.contains('dragging') || c === placeholder);
    const visibleIndex = placeholder ? siblings.indexOf(placeholder) : siblings.length;
    const col = board.columns.find((c) => c.id === zone.dataset.col);
    const visibleCards = col.cards.filter((c) => matches(c) && c.id !== cardId);
    const modelIndex = visibleIndex >= visibleCards.length
      ? col.cards.length
      : col.cards.indexOf(visibleCards[visibleIndex]);
    clearPlaceholders();
    moveCard(cardId, col.id, modelIndex);
  });
}

// ---------- dialog ----------

let editingId = null;
let newCardCol = null;

function openDialog(cardId, colId = null) {
  editingId = cardId;
  newCardCol = colId;
  form.reset();

  const select = form.elements.column;
  select.textContent = '';
  for (const col of board.columns) {
    const opt = document.createElement('option');
    opt.value = col.id;
    opt.textContent = col.title;
    select.appendChild(opt);
  }

  const meta = $('#dialog-meta');
  if (cardId) {
    const { col, card } = findCard(cardId);
    if (card.unreviewed) {
      card.unreviewed = false;
      scheduleSave();
    }
    $('#dialog-title').textContent = 'Edit card';
    for (const k of ['company', 'role', 'location', 'salary', 'url', 'contact', 'notes']) form.elements[k].value = card[k] || '';
    select.value = col.id;
    const added = card.created_at ? new Date(card.created_at).toLocaleDateString() : '?';
    const stage = daysSince(card.stage_changed_at);
    meta.textContent = `Added ${added}` + (stage !== null ? ` · ${stage} day${stage === 1 ? '' : 's'} in ${col.title}` : '');
    $('#delete-btn').hidden = false;
  } else {
    $('#dialog-title').textContent = 'New card';
    select.value = colId || board.columns[0].id;
    meta.textContent = '';
    $('#delete-btn').hidden = true;
  }
  dialog.showModal();
  form.elements.company.focus();
}

form.addEventListener('submit', (e) => {
  e.preventDefault();
  const data = Object.fromEntries(new FormData(form).entries());
  const targetCol = data.column;
  delete data.column;
  for (const k in data) data[k] = data[k].trim();

  if (editingId) {
    const found = findCard(editingId);
    Object.assign(found.card, data);
    if (found.col.id !== targetCol) {
      found.col.cards.splice(found.idx, 1);
      found.card.stage_changed_at = now();
      board.columns.find((c) => c.id === targetCol).cards.push(found.card);
    }
  } else {
    const card = { id: uid(), ...data, created_at: now(), stage_changed_at: now() };
    board.columns.find((c) => c.id === targetCol).cards.push(card);
  }
  dialog.close();
  render();
  scheduleSave();
});

$('#cancel-btn').addEventListener('click', () => dialog.close());
dialog.addEventListener('close', render);

$('#fetch-btn').addEventListener('click', async () => {
  const url = form.elements.url.value.trim();
  const meta = $('#dialog-meta');
  if (!url) { meta.textContent = 'Enter a posting URL first.'; return; }
  meta.textContent = 'Fetching…';
  try {
    const info = await store.fetchJob(url);
    let cleaned = false;
    if (info.url && info.url !== url) {
      form.elements.url.value = info.url;
      cleaned = true;
    }
    let filled = 0;
    for (const k of ['company', 'role', 'location', 'salary']) {
      if (!form.elements[k].value.trim() && info[k]) { form.elements[k].value = info[k]; filled++; }
    }
    meta.textContent = (filled ? `Filled ${filled} field${filled === 1 ? '' : 's'} from the page.`
      : (info.title ? `Nothing new to fill. Page title: ${info.title}` : 'Could not find job details on that page.'))
      + (cleaned ? ' Removed tracking parameters from the link.' : '');
  } catch (e) {
    meta.textContent = 'Fetch failed: ' + e;
  }
});
$('#delete-btn').addEventListener('click', () => {
  const found = findCard(editingId);
  if (!found) return dialog.close();
  if (!confirm(`Delete "${found.card.company || 'this card'}"?`)) return;
  found.col.cards.splice(found.idx, 1);
  dialog.close();
  render();
  scheduleSave();
});

// ---------- top bar ----------

$('#search').addEventListener('input', (e) => {
  filter = e.target.value.trim().toLowerCase();
  render();
});

$('#data-path').addEventListener('click', async () => {
  try {
    setStatus(await store.path());
  } catch (e) {
    setStatus(String(e), true);
  }
});

document.addEventListener('keydown', (e) => {
  const anyDialogOpen = document.querySelector('dialog[open]');
  if (anyDialogOpen || document.activeElement === $('#search')) return;
  if (e.key === 'n') {
    e.preventDefault();
    openDialog(null, 'wishlist');
  } else if (e.key === 't') {
    e.preventDefault();
    openTriage();
  }
});

// ---------- inbox (cards arriving from Discord etc.) ----------

async function drainInbox() {
  let cards;
  try {
    cards = await store.takeInbox();
  } catch (e) {
    setStatus('Inbox error: ' + e, true);
    return;
  }
  if (!cards.length) return;
  const inbox = column('inbox') || board.columns[0];
  let direct = 0;
  for (const card of cards) {
    if (findCard(card.id)) continue;
    card.stage_changed_at = card.stage_changed_at || now();
    // An inbox file may name a stage to skip triage (used by scripts and manual imports).
    const target = card.stage ? column(card.stage) : null;
    delete card.stage;
    if (target) {
      direct++;
      target.cards.unshift(card);
    } else {
      inbox.cards.unshift(card);
    }
  }
  render();
  if (triageDialog.open) showTriageCard();
  scheduleSave();
  const queued = cards.length - direct;
  setStatus(queued ? `${queued} new card${queued === 1 ? '' : 's'} in Inbox` : `Added ${direct} card${direct === 1 ? '' : 's'}`);
}

// ---------- triage ----------

const triageDialog = $('#triage-dialog');
let triageIndex = 0;
let triageUndo = null; // { card, colId, index }

function openTriage() {
  if (!inboxCards().length) return;
  triageIndex = 0;
  triageUndo = null;
  showTriageCard();
  triageDialog.showModal();
}

function showTriageCard() {
  const cards = inboxCards();
  if (!cards.length) {
    triageDialog.close();
    setStatus('Inbox clear');
    return;
  }
  if (triageIndex >= cards.length) triageIndex = 0;
  const card = cards[triageIndex];
  if (card.unreviewed) {
    card.unreviewed = false;
    scheduleSave();
  }
  $('#triage-progress').textContent = `${triageIndex + 1} of ${cards.length}`;
  $('#t-company').textContent = card.company || 'Untitled';
  $('#t-role').textContent = card.role || '';
  $('#t-line').textContent = [card.location, card.salary, card.contact].filter(Boolean).join(' · ');
  const a = $('#t-url');
  a.textContent = card.url || '';
  a.hidden = !card.url;
  $('#t-notes').textContent = card.notes || '';
  $('#t-notes').hidden = !card.notes;
  $('#triage-undo-wrap').hidden = !triageUndo;
}

function triageAct(act) {
  const cards = inboxCards();
  const card = cards[triageIndex];
  if (!card) return;
  const inbox = column('inbox');

  const moveTo = (colId) => {
    inbox.cards.splice(triageIndex, 1);
    card.stage_changed_at = now();
    column(colId).cards.unshift(card);
    triageUndo = { card, colId, index: triageIndex };
  };

  switch (act) {
    case 'open':
      if (card.url) store.openUrl(card.url).catch((e) => setStatus('Could not open link: ' + e, true));
      return;
    case 'edit':
      triageDialog.close();
      openDialog(card.id);
      return;
    case 'later':
      triageIndex = (triageIndex + 1) % cards.length;
      showTriageCard();
      return;
    case 'keep':
      moveTo('wishlist');
      break;
    case 'apply':
      moveTo('applied');
      break;
    case 'pass':
      inbox.cards.splice(triageIndex, 1);
      triageUndo = { card, colId: null, index: triageIndex };
      break;
    case 'undo':
      if (!triageUndo) return;
      if (triageUndo.colId) {
        const col = column(triageUndo.colId);
        col.cards.splice(col.cards.indexOf(triageUndo.card), 1);
      }
      inbox.cards.splice(Math.min(triageUndo.index, inbox.cards.length), 0, triageUndo.card);
      triageIndex = triageUndo.index;
      triageUndo = null;
      break;
    default:
      return;
  }
  render();
  scheduleSave();
  if (triageDialog.open || act !== 'undo') showTriageCard();
}

$('#triage-btn').addEventListener('click', openTriage);
$('#triage-close').addEventListener('click', () => triageDialog.close());
$('#triage-undo').addEventListener('click', () => triageAct('undo'));
triageDialog.querySelectorAll('[data-act]').forEach((b) => b.addEventListener('click', () => triageAct(b.dataset.act)));
$('#t-url').addEventListener('click', (e) => {
  e.preventDefault();
  triageAct('open');
});
triageDialog.addEventListener('close', render);
triageDialog.addEventListener('keydown', (e) => {
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  const keys = { o: 'open', e: 'edit', p: 'pass', l: 'later', k: 'keep', a: 'apply', u: 'undo', ArrowRight: 'later' };
  const act = keys[e.key];
  if (act) {
    e.preventDefault();
    triageAct(act);
  }
});

// ---------- settings dialog ----------

const settingsDialog = $('#settings-dialog');
const settingsForm = $('#settings-form');
const settingsStatus = $('#settings-status');

async function openSettings() {
  settingsForm.reset();
  settingsStatus.textContent = '';
  try {
    const s = await store.getSettings();
    settingsForm.elements.discord_token.value = s.discord_token || '';
    settingsForm.elements.discord_channel_id.value = s.discord_channel_id || '';
    const st = await store.discordStatus();
    settingsStatus.textContent = describeStatus(st);
  } catch (e) {
    settingsStatus.textContent = String(e);
  }
  settingsDialog.showModal();
}

function describeStatus(st) {
  if (!st.configured) return 'Not configured yet.';
  if (st.last_error) return 'Last check failed: ' + st.last_error;
  const when = st.last_poll ? new Date(st.last_poll).toLocaleTimeString() : 'never';
  return `Working. Last checked ${when}; ${st.captured} link${st.captured === 1 ? '' : 's'} captured this session.`;
}

$('#settings-btn').addEventListener('click', openSettings);
$('#settings-cancel-btn').addEventListener('click', () => settingsDialog.close());

$('#test-discord-btn').addEventListener('click', async () => {
  const token = settingsForm.elements.discord_token.value.trim();
  const channelId = settingsForm.elements.discord_channel_id.value.trim();
  if (!token || !channelId) { settingsStatus.textContent = 'Enter both the bot token and the channel ID.'; return; }
  settingsStatus.textContent = 'Testing…';
  try {
    settingsStatus.textContent = await store.testDiscord(token, channelId);
  } catch (e) {
    settingsStatus.textContent = String(e);
  }
});

settingsForm.addEventListener('submit', async (e) => {
  e.preventDefault();
  try {
    await store.saveSettings({
      discord_token: settingsForm.elements.discord_token.value.trim(),
      discord_channel_id: settingsForm.elements.discord_channel_id.value.trim(),
    });
    settingsDialog.close();
    setStatus('Discord settings saved');
  } catch (err) {
    settingsStatus.textContent = 'Save failed: ' + err;
  }
});

// Links inside the setup instructions open in the system browser.
settingsDialog.addEventListener('click', (e) => {
  const a = e.target.closest('a[data-url]');
  if (!a) return;
  e.preventDefault();
  store.openUrl(a.dataset.url).catch((err) => (settingsStatus.textContent = 'Could not open link: ' + err));
});

// ---------- boot ----------

(async () => {
  try {
    board = await store.load();
    if (!board.columns?.length) board = defaultBoard();
    ensureInboxColumn(board);
    setStatus('');
  } catch (e) {
    setStatus('Load failed: ' + e, true);
  }
  render();
  await drainInbox();
  store.onInbox(drainInbox);
})();
