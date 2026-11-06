// Friends, shared with Orbit Client (the same server, sign-in, requests and messages).
// Polls in the background for the top bar's badge and notifications; the Friends page
// asks for faster polls while it's open. While no game runs, the poll also shows you
// online "in Orbit Launcher" to your friends (the backend decides).
import * as api from './api.js';
import { store, activeAccount } from './store.js';
import { errorText } from './tauri.js';
import { toast } from './ui.js';

const SLOW_MS = 15000;
const FAST_MS = 3000;
const HISTORY = 200;

let timer = null;
let fast = false;
let polling = false;
let lastMessage = 0;
/** Refetching every chat after the server's message numbers went back: no pop-ups for old messages. */
let resyncing = false;
let account = null;
/** The chat on screen: its messages aren't unread and don't pop up. */
let openChat = null;

/** "Online", "Playing on X", "In singleplayer", "In Orbit Launcher" or "Offline". */
export function friendStatus(friend) {
  if (!friend.online) return 'Offline';
  if (friend.server === '') return 'Online';
  if (friend.server === 'singleplayer') return 'In singleplayer';
  if (friend.server === 'launcher') return 'In Orbit Launcher';
  return `Playing on ${friend.server}`;
}

/** A server address friends can join. */
export const joinable = (friend) => friend.online && !['', 'singleplayer', 'launcher'].includes(friend.server);

export const friendName = (person) => person.name || person.uuid.slice(0, 8);

export const totalUnread = () => [...store.state.unread.values()].reduce((sum, n) => sum + n, 0);

export function startFriends() {
  schedule(1000);
}

/** The Friends page polls every few seconds while open. */
export function setFast(on) {
  fast = on;
  if (on) schedule(0);
}

export function setOpenChat(uuid) {
  openChat = uuid;
  if (uuid && store.state.unread.has(uuid)) {
    store.state.unread.delete(uuid);
    store.set('unread', store.state.unread);
  }
}

function schedule(ms) {
  clearTimeout(timer);
  timer = setTimeout(poll, ms);
}

function reset() {
  lastMessage = 0;
  store.set('chats', new Map());
  store.set('unread', new Map());
  store.set('social', { friends: [], incoming: [], outgoing: [], blocked: [], loaded: false, error: '' });
}

async function poll() {
  if (polling) return;
  const me = activeAccount()?.uuid ?? null;
  if (me !== account) {
    // Another account: its friends, not the last one's.
    account = me;
    reset();
  }
  if (!me) {
    schedule(SLOW_MS);
    return;
  }
  polling = true;
  try {
    const result = await api.social.poll(lastMessage);
    if (account === me) apply(me, result);
  } catch (error) {
    store.set('social', { ...store.state.social, error: errorText(error) });
  } finally {
    polling = false;
    schedule(fast ? FAST_MS : SLOW_MS);
  }
}

/** Run now (after an action), so the change shows at once. */
export const refreshFriends = () => schedule(0);

function apply(me, result) {
  const before = store.state.social;
  const notify = before.loaded;
  const dashed = (uuid) => uuid.replace(/-/g, '');
  const self = dashed(me);

  const friends = [...(result.friends ?? [])].sort((a, b) =>
    a.online !== b.online ? (a.online ? -1 : 1) : friendName(a).localeCompare(friendName(b), undefined, { sensitivity: 'base' }));

  if (notify) {
    const old = new Map(before.friends.map((f) => [f.uuid, f]));
    const asked = new Set(before.outgoing.map((p) => p.uuid));
    for (const friend of friends) {
      const was = old.get(friend.uuid);
      if (!was && asked.has(friend.uuid)) toast(`${friendName(friend)} accepted your friend request`, { kind: 'success' });
      else if (was && !was.online && friend.online) toast(`${friendName(friend)} is online`, { description: friendStatus(friend) });
    }
    const requested = new Set(before.incoming.map((p) => p.uuid));
    for (const person of result.incoming ?? []) {
      if (!requested.has(person.uuid)) {
        toast(`Friend request from ${friendName(person)}`, {
          actions: [{ label: 'Open Friends', onClick: () => (location.hash = '#/friends') }],
        });
      }
    }
  }

  const names = new Map(friends.map((f) => [f.uuid, friendName(f)]));
  const chats = store.state.chats;
  const unread = store.state.unread;
  // Below what we've seen means the server lost its message file and numbers from 1 again:
  // start the chats over, or every new message would be skipped as already seen.
  const restarted = typeof result.lastMessage === 'number' && result.lastMessage < lastMessage;
  if (restarted) {
    lastMessage = 0;
    chats.clear();
    refreshFriends();
  }
  const quiet = restarted || resyncing;
  resyncing = restarted;
  for (const message of result.messages ?? []) {
    if (message.id <= lastMessage) continue;
    lastMessage = message.id;
    const mine = dashed(message.from) === self;
    const other = mine ? message.to : message.from;
    const chat = chats.get(other) ?? [];
    chat.push(message);
    if (chat.length > HISTORY) chat.shift();
    chats.set(other, chat);
    if (!mine && notify && !quiet && other !== openChat) {
      unread.set(other, (unread.get(other) ?? 0) + 1);
      toast(names.get(other) ?? 'A friend', {
        description: message.text,
        actions: [{ label: 'Reply', onClick: () => (location.hash = `#/friends?chat=${other}`) }],
      });
    }
  }
  // Chats with people who aren't friends anymore are dropped.
  const ids = new Set(friends.map((f) => f.uuid));
  for (const key of [...chats.keys()]) if (!ids.has(key)) chats.delete(key);
  for (const key of [...unread.keys()]) if (!ids.has(key)) unread.delete(key);

  store.set('chats', chats);
  store.set('unread', unread);
  store.set('social', {
    friends,
    incoming: result.incoming ?? [],
    outgoing: result.outgoing ?? [],
    blocked: result.blocked ?? [],
    loaded: true,
    error: '',
  });
}

/** Run an action; toasts its error, or `success` when it worked, then refreshes. */
export async function friendAction(run, success) {
  try {
    await run();
    if (success) toast(success, { kind: 'success' });
    return true;
  } catch (error) {
    toast(errorText(error), { kind: 'error' });
    return false;
  } finally {
    refreshFriends();
  }
}
