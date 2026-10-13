// Public product name and slug. Internal identifiers (package names, storage
// names, event names) keep their original values; see docs/release/identity.md.
export const PRODUCT_NAME = 'TrickTic Dictate';
export const PRODUCT_SLUG = 'tricktic-dictate';

// Public legal pages and support address. The desktop shell opens them from its
// own copies in apps/desktop/tauri/src/product.rs; a test keeps the two equal.
export const LEGAL_BASE = 'https://legal.tricktic.com/tricktic-dictate';
export const SUPPORT_EMAIL = 'contact@tricktic.com';
export const LEGAL_LINKS = {
  privacy: `${LEGAL_BASE}/privacy`,
  terms: `${LEGAL_BASE}/terms`,
  security: `${LEGAL_BASE}/security`,
  support: `mailto:${SUPPORT_EMAIL}`,
} as const;
