// Friends: the same friends, requests and chats as in Orbit Client. Left: friends (online first),
// requests or blocked players, and a box to add someone by Minecraft name. Right: the chat with
// the selected friend, with Join (launches Orbit Client into their server), Remove and Block.
import * as api from '../api.js';
import { playOrbitClient } from '../actions.js';
import { friendAction, friendName, friendStatus, joinable, refreshFriends, setFast, setOpenChat } from '../friends.js';
import { store, activeAccount, prefs } from '../store.js';
import { confirmDialog, emptyState, html, icon, playerHead, setHtml } from '../ui.js';

const TABS = ['friends', 'requests', 'blocked'];
const time = (ms) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

export function render(root, _params, query) {
  let tab = prefs.get('friends.tab', 'friends');
  if (!TABS.includes(tab)) tab = 'friends';
  let selected = query.get('chat') ?? prefs.get('friends.chat', null);
  if (query.get('chat')) tab = 'friends';

  setHtml(
    root,
    html`
      <header class="page-head">
        <div>
          <h1>Friends</h1>
          <p>The same friends as in Orbit Client: see who's online, chat, and join their server.</p>
        </div>
      </header>
      <section class="card friends-card">
        <aside class="friends-side">
          <div class="segmented friends-tabs" data-tabs></div>
          <form class="friends-add" data-add>
            <input class="input" data-name maxlength="16" placeholder="Add by Minecraft name" autocomplete="off" spellcheck="false">
            <button class="btn btn-primary" type="submit">${icon('plus', 'icon-sm')} Add</button>
          </form>
          <div class="friends-list" data-list></div>
          <div class="small muted friends-error" data-error></div>
        </aside>
        <div class="friends-chat" data-chat></div>
      </section>`,
  );

  const tabsEl = root.querySelector('[data-tabs]');
  const listEl = root.querySelector('[data-list]');
  const chatEl = root.querySelector('[data-chat]');
  const errorEl = root.querySelector('[data-error]');
  const nameInput = root.querySelector('[data-name]');

  const person = (p, actions, extra = '') => html`
    <div class="friend-row ${extra}" data-select="${p.uuid}">
      <span class="friend-head">${playerHead({ uuid: p.uuid, username: friendName(p) }, 32)}</span>
      <span class="grow" style="min-width:0">
        <b class="truncate">${friendName(p)}</b>
      </span>
      ${actions}
    </div>`;

  const drawTabs = () => {
    const { friends, incoming } = store.state.social;
    const online = friends.filter((f) => f.online).length;
    setHtml(
      tabsEl,
      html`
        <button data-tab="friends" aria-pressed="${tab === 'friends'}">Friends <span class="faint">${online}/${friends.length}</span></button>
        <button data-tab="requests" aria-pressed="${tab === 'requests'}">Requests ${incoming.length ? html`<span class="badge badge-critical">${incoming.length}</span>` : ''}</button>
        <button data-tab="blocked" aria-pressed="${tab === 'blocked'}">Blocked</button>`,
    );
  };

  const drawList = () => {
    const { friends, incoming, outgoing, blocked, loaded, error } = store.state.social;
    errorEl.textContent = error;
    if (!activeAccount()) {
      setHtml(listEl, emptyState({ iconName: 'user', title: 'Sign in to see your friends' }));
      return;
    }
    if (!loaded) {
      setHtml(listEl, html`<div class="small muted" style="padding:12px">${error ? 'Friends are unavailable right now.' : 'Loading friends…'}</div>`);
      return;
    }
    if (tab === 'friends') {
      setHtml(
        listEl,
        friends.length
          ? html`${friends.map((f) => {
              const unread = store.state.unread.get(f.uuid) ?? 0;
              return html`
                <button class="friend-row ${f.uuid === selected ? 'is-on' : ''} ${f.online ? '' : 'is-offline'}" data-select="${f.uuid}">
                  <span class="friend-head">${playerHead({ uuid: f.uuid, username: friendName(f) }, 32)}<i class="friend-dot ${f.online ? 'on' : ''}"></i></span>
                  <span class="grow" style="min-width:0">
                    <b class="truncate">${friendName(f)}</b>
                    <small class="truncate ${f.online ? (joinable(f) ? 'is-join' : 'is-online') : ''}">${friendStatus(f)}</small>
                  </span>
                  ${unread ? html`<span class="badge badge-critical">${unread}</span>` : ''}
                </button>`;
            })}`
          : html`<div class="small muted" style="padding:12px">No friends yet. Add someone by their Minecraft name: they accept in Orbit Client or here.</div>`,
      );
    } else if (tab === 'requests') {
      setHtml(
        listEl,
        html`
          <div class="friends-section">Incoming</div>
          ${incoming.length
            ? incoming.map((p) => person(p, html`
                <button class="btn btn-sm btn-primary" data-accept="${p.uuid}">${icon('check', 'icon-sm')} Accept</button>
                <button class="btn btn-sm btn-ghost" data-decline="${p.uuid}" title="Decline">${icon('x', 'icon-sm')}</button>`))
            : html`<div class="small muted friends-none">No requests.</div>`}
          <div class="friends-section">Sent</div>
          ${outgoing.length
            ? outgoing.map((p) => person(p, html`<button class="btn btn-sm btn-ghost" data-cancel="${p.uuid}">Cancel</button>`))
            : html`<div class="small muted friends-none">No pending requests.</div>`}`,
      );
    } else {
      setHtml(
        listEl,
        blocked.length
          ? html`${blocked.map((p) => person(p, html`<button class="btn btn-sm btn-outline" data-unblock="${p.uuid}">Unblock</button>`))}`
          : html`<div class="small muted" style="padding:12px">Nobody blocked. Blocked players can't add or message you.</div>`,
      );
    }
  };

  let renderedChat = null;
  const drawChat = () => {
    const friend = tab === 'friends' ? store.state.social.friends.find((f) => f.uuid === selected) : null;
    setOpenChat(friend?.uuid ?? null);
    if (!friend) {
      renderedChat = null;
      setHtml(chatEl, emptyState({
        iconName: 'user',
        title: tab === 'friends' ? 'Pick a friend to chat' : tab === 'requests' ? 'Friend requests' : 'Blocked players',
        description: tab === 'friends'
          ? 'Messages are shared with Orbit Client and kept on every device you sign in on.'
          : tab === 'requests' ? 'Requests reach players the next time they open Orbit Client or the launcher.'
          : "Blocked players can't send you requests or messages.",
      }));
      return;
    }
    const messages = store.state.chats.get(friend.uuid) ?? [];
    const me = (activeAccount()?.uuid ?? '').replace(/-/g, '');
    const draft = chatEl.querySelector('[data-text]')?.value ?? '';
    const list = chatEl.querySelector('[data-messages]');
    const atBottom = !list || list.scrollHeight - list.scrollTop - list.clientHeight < 40;
    setHtml(
      chatEl,
      html`
        <div class="friends-chat-head">
          <span class="friend-head">${playerHead({ uuid: friend.uuid, username: friendName(friend) }, 36)}<i class="friend-dot ${friend.online ? 'on' : ''}"></i></span>
          <div class="grow" style="min-width:0">
            <h2 class="truncate">${friendName(friend)}</h2>
            <small class="truncate ${friend.online ? (joinable(friend) ? 'is-join' : 'is-online') : 'muted'}">${friendStatus(friend)}</small>
          </div>
          ${joinable(friend) && html`<button class="btn btn-sm btn-primary" data-join title="Launch Orbit Client into ${friend.server}">${icon('play', 'icon-sm')} Join</button>`}
          <button class="btn btn-sm btn-outline" data-remove>Remove</button>
          <button class="btn btn-sm btn-danger" data-block>Block</button>
        </div>
        <div class="friends-messages" data-messages>
          ${messages.length
            ? messages.map((m) => {
                const mine = m.from.replace(/-/g, '') === me;
                return html`<div class="bubble ${mine ? 'mine' : ''}"><span class="selectable">${m.text}</span><small>${time(m.time)}</small></div>`;
              })
            : html`<div class="small muted friends-hello">Say hi to ${friendName(friend)}!</div>`}
        </div>
        <form class="friends-send" data-send>
          <input class="input" data-text maxlength="256" placeholder="Message ${friendName(friend)}" autocomplete="off">
          <button class="btn btn-primary" type="submit">Send</button>
        </form>`,
    );
    const input = chatEl.querySelector('[data-text]');
    input.value = draft;
    if (renderedChat !== friend.uuid || document.activeElement === document.body) input.focus();
    const newList = chatEl.querySelector('[data-messages]');
    if (atBottom || renderedChat !== friend.uuid) newList.scrollTop = newList.scrollHeight;
    renderedChat = friend.uuid;
  };

  // The chat keeps its input focused and its draft across redraws; only redraw it when its data changes.
  let chatKey = '';
  const drawChatIfChanged = () => {
    const friend = store.state.social.friends.find((f) => f.uuid === selected);
    const key = JSON.stringify([tab, selected, friend, (store.state.chats.get(selected) ?? []).length]);
    if (key === chatKey) return;
    chatKey = key;
    drawChat();
  };

  const draw = () => {
    drawTabs();
    drawList();
    drawChatIfChanged();
  };

  root.addEventListener('click', async (event) => {
    const t = event.target;
    const tabBtn = t.closest('[data-tab]');
    if (tabBtn) {
      tab = tabBtn.dataset.tab;
      prefs.set('friends.tab', tab);
      return draw();
    }
    const uuidOf = (attr) => t.closest(`[${attr}]`)?.getAttribute(attr);
    let uuid;
    if ((uuid = uuidOf('data-accept'))) return friendAction(() => api.social.accept(uuid), 'Friend request accepted');
    if ((uuid = uuidOf('data-decline'))) return friendAction(() => api.social.remove(uuid), 'Request declined');
    if ((uuid = uuidOf('data-cancel'))) return friendAction(() => api.social.remove(uuid), 'Request cancelled');
    if ((uuid = uuidOf('data-unblock'))) return friendAction(() => api.social.block(uuid, false), 'Unblocked');
    const friend = store.state.social.friends.find((f) => f.uuid === selected);
    if (t.closest('[data-join]') && friend) return playOrbitClient({ type: 'multiplayer', address: friend.server });
    if (t.closest('[data-remove]') && friend) {
      const ok = await confirmDialog({ title: `Remove ${friendName(friend)}?`, message: 'You can add each other again later.', confirmLabel: 'Remove', danger: true });
      if (ok) friendAction(() => api.social.remove(friend.uuid), `Removed ${friendName(friend)}`);
      return;
    }
    if (t.closest('[data-block]') && friend) {
      const ok = await confirmDialog({
        title: `Block ${friendName(friend)}?`,
        message: "This removes them from your friends. They can't send you requests or messages until you unblock them.",
        confirmLabel: 'Block',
        danger: true,
      });
      if (ok) friendAction(() => api.social.block(friend.uuid, true), `Blocked ${friendName(friend)}`);
      return;
    }
    const row = t.closest('button.friend-row[data-select]');
    if (row) {
      selected = row.dataset.select;
      prefs.set('friends.chat', selected);
      draw();
    }
  });

  root.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (event.target.matches('[data-add]')) {
      const name = nameInput.value.trim();
      if (!/^[A-Za-z0-9_]{1,16}$/.test(name)) return nameInput.focus();
      if (await friendAction(() => api.social.request(name), `Friend request sent to ${name}`)) nameInput.value = '';
    } else if (event.target.matches('[data-send]')) {
      const input = event.target.querySelector('[data-text]');
      const text = input.value.trim();
      if (!text || !selected) return;
      input.value = '';
      const sent = await friendAction(() => api.social.message(selected, text));
      if (!sent && !input.value) input.value = text;
    }
  });

  setFast(true);
  refreshFriends();
  draw();
  const off = store.on(['social', 'chats', 'unread', 'accounts'], draw);
  return () => {
    off();
    setFast(false);
    setOpenChat(null);
  };
}
