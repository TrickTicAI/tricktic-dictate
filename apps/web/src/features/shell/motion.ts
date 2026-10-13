import { useEffect } from 'react';

/*
 * One Animations setting for the whole application.
 *
 * On the desktop it is Writer's own preference (`reduced_motion`, the field the
 * floating panel already followed), not the operating system's animation
 * switch: Windows reports "animation effects off" to WebView2 as
 * `prefers-reduced-motion: reduce`, and that used to freeze every progress
 * indicator in the window half-way, which reads as a hung app. The browser
 * client has no Writer preference store, so there the system setting decides.
 *
 * The choice is written to `<html data-motion>`. Styles reduce motion under
 * `[data-motion='reduced']`; indicators that report progress keep moving, in
 * steps, so a reduced window never looks stuck.
 */
export type Motion = 'full' | 'reduced';

export const currentMotion = (): Motion =>
  document.documentElement.dataset.motion === 'reduced' ? 'reduced' : 'full';

export function applyMotion(motion: Motion) {
  document.documentElement.dataset.motion = motion;
}

/**
 * Keeps `<html data-motion>` in step with Writer's preference when there is one
 * (`desktop` is the loaded preference, `undefined` while loading), and with the
 * system setting otherwise.
 */
export function useMotion(desktopReduced: boolean | undefined, hasDesktopPreference: boolean) {
  useEffect(() => {
    if (hasDesktopPreference) {
      if (desktopReduced !== undefined) applyMotion(desktopReduced ? 'reduced' : 'full');
      return;
    }
    const query = matchMedia('(prefers-reduced-motion: reduce)');
    const follow = () => applyMotion(query.matches ? 'reduced' : 'full');
    follow();
    query.addEventListener('change', follow);
    return () => query.removeEventListener('change', follow);
  }, [desktopReduced, hasDesktopPreference]);
}
