// Sidebar footer: the active player, with account switching, skin and sign-out.
import { signIn, signOut, switchAccount } from '../actions.js';
import { activeAccount, store } from '../store.js';
import { html, icon, playerHead, setHtml, showMenu } from '../ui.js';

export function mountAccountMenu(root) {
  const render = () => {
    const account = activeAccount();
    if (!account) {
      setHtml(root, html`<button class="btn btn-primary btn-block" data-signin>${icon('user')} Sign in with Microsoft</button>`);
      return;
    }
    setHtml(
      root,
      html`<button class="account-card" data-menu title="Accounts">
        ${playerHead(account, 34)}
        <div class="grow">
          <div class="truncate bold">${account.username}</div>
          <div class="tiny muted">Microsoft account</div>
        </div>
        ${icon('chevron-up', 'icon-sm')}
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
        { header: 'Accounts' },
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
      { placement: 'above' },
    );
  });

  render();
  return store.on('accounts', render);
}
