// The chat the agent panel shows, for components below App that act for it (the transcript's image viewer, the skills). Null-free:
// outside an App (a static render, a test) it is the default chat.

import { createContext } from 'react';
import { CHAT_ID } from './api';

export const ActiveChatContext = createContext<string>(CHAT_ID);
