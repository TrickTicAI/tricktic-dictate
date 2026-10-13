import { createRoot } from 'react-dom/client';
import { ModeSettings } from '../ModeSettings';
import { ProcessingPanel } from '../ProcessingPanel';
createRoot(document.getElementById('root')!).render(<><ModeSettings /><ProcessingPanel jobId="11111111-1111-4111-8111-111111111111" /></>);
