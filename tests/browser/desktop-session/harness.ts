import { SessionOwner } from '../../../apps/web/src/features/desktop-session/runtime/owner';
import { installOwnerBridge } from '../../../apps/web/src/features/desktop-session/runtime/bridge';
const owner = await SessionOwner.create({ settings: () => ({ language: 'en', options: null }) });
(window as any).sessionFixture = { owner, levels: 0 };
owner.subscribe(event => { if (event.kind === 'level') (window as any).sessionFixture.levels++; });
installOwnerBridge(owner);
