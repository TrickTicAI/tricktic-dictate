import React from 'react';
import { createRoot } from 'react-dom/client';
import { UpdatesPanel,useUpdates } from '../updates';
function Harness() { const updates=useUpdates(); return <UpdatesPanel updates={updates}/>; }
createRoot(document.getElementById('root')!).render(<Harness/>);
