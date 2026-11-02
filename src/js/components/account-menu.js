// Right of the top bar: the active player's head and name, opening account
// switching, skin and sign-out.
import { signIn, signOut, switchAccount } from '../actions.js';
import { activeAccount, store } from '../store.js';
import { html, icon, playerHead, setHtml, showMenu } from '../ui.js';

export function mountAccountMenu(root) {
  const render = () => {
    const account = activeAccount();
    if (!account) {
      setHtml(
        root,
        html`<button class="account-btn is-signin" data-signin aria-label="Sign in with Microsoft">${icon('log-in', 'icon-sm')} Sign in</button>`,
      );
      return;
    }
    setHtml(
      root,
      html`<button class="account-btn" data-menu aria-label="Account: ${account.username}">
        ${playerHead(account, 26)}<span>${account.username}</span>
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
      { align: 'end' },
    );
  });

  render();
  return store.on('accounts', render);
}
