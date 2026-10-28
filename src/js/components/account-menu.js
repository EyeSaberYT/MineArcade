// Bottom of the rail: the active player's head, opening account switching,
// skin and sign-out (like the profile button in the Modrinth App).
import { signIn, signOut, switchAccount } from '../actions.js';
import { activeAccount, store } from '../store.js';
import { html, icon, playerHead, setHtml, showMenu } from '../ui.js';

export function mountAccountMenu(root) {
  const render = () => {
    const account = activeAccount();
    if (!account) {
      setHtml(
        root,
        html`<button class="rail-btn rail-signin" data-signin data-tip="Sign in with Microsoft" aria-label="Sign in with Microsoft">${icon('log-in')}</button>`,
      );
      return;
    }
    setHtml(
      root,
      html`<button class="rail-btn rail-account" data-menu data-tip="Playing as ${account.username}" aria-label="Account: ${account.username}">
        ${playerHead(account, 32)}
      </button>`,
    );
  };

  root.addEventListener('click', (event) => {
    if (event.target.closest('[data-signin]')) return signIn();
    const anchor = event.target.closest('[data-menu]');
    if (!anchor) return;
    const current = activeAccount();
    showMenu(
      anchor,
      [
        { header: 'Playing as' },
        ...store.state.accounts.map((account) => ({
          label: account.username,
          leading: String(playerHead(account, 22)),
          trail: account.active ? 'Active' : '',
          onSelect: () => !account.active && switchAccount(account.uuid),
        })),
        { separator: true },
        {
          label: 'Skin',
          icon: 'shirt',
          onSelect: async () => (await import('../dialogs/skin.js')).openSkinDialog(),
        },
        { label: 'Add account', icon: 'plus', onSelect: signIn },
        current && { label: `Sign out ${current.username}`, icon: 'log-out', danger: true, onSelect: () => signOut(current) },
      ].filter(Boolean),
      { placement: 'right' },
    );
  });

  render();
  return store.on('accounts', render);
}
