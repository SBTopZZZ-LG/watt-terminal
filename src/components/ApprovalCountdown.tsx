// The approval countdown: a muted line under a pending approval card's actions, ticking once a second
// until it runs out. With no expiry (a card from an older session) it renders nothing.

import { useEffect, useState } from 'react';
import { remainingLabel } from '../lib/approvalTime';

export function ApprovalCountdown({ expiresAt }: { expiresAt?: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (expiresAt === undefined) {
      return;
    }
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [expiresAt]);
  if (expiresAt === undefined) {
    return null;
  }
  return <div className="approval-countdown">{remainingLabel(expiresAt, now)}</div>;
}
