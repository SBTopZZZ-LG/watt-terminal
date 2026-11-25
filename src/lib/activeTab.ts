// The terminal the UI shows, for components that sit below App without a prop path (FindBar, ContextMenu, GridChip). Null
// where no terminal is mounted (static renders in tests).

import { createContext } from 'react';
import type { TabController } from './tab';

export const ActiveTabContext = createContext<TabController | null>(null);
