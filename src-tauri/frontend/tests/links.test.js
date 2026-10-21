import { describe, expect, it, vi } from 'vitest'

// links.js opens pages through the Tauri shell and reads the language from i18n.js, which talks
// to the backend on import. Neither exists outside the app.
vi.mock('@tauri-apps/plugin-shell', () => ({ open: vi.fn() }));
vi.mock('../i18n.js', () => ({ language: () => 'es' }));

const { open } = await import('@tauri-apps/plugin-shell');
const { SITE_PAGES, openSitePage, siteUrl } = await import('../links.js');

describe('site links', () => {
  it('put the language first, the way the site routes them', () => {
    expect(siteUrl('discordSetup', 'es')).toBe('https://www.mechardo3d.xyz/es/ds2000/discord-setup');
    expect(siteUrl('discordSetup', 'en')).toBe('https://www.mechardo3d.xyz/en/ds2000/discord-setup');
    expect(siteUrl('home', 'en')).toBe('https://www.mechardo3d.xyz/en');
  });

  it('open in the language the app is in', () => {
    openSitePage('termsOfService');
    expect(open).toHaveBeenCalledWith('https://www.mechardo3d.xyz/es/ds2000/terms-of-service');
  });

  it('only use paths the site has a route for', () => {
    // From the site's router (Mechanix97/mechardo3d, src/main.rs).
    const routes = ['', '/ds2000', '/ds2000/discord-setup', '/ds2000/terms-of-service', '/ds2000/privacy-policy'];
    for (const path of Object.values(SITE_PAGES)) {
      expect(routes).toContain(path);
    }
  });
});
