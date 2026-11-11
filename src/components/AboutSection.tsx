import wattMark from '../assets/watt-mark.png';
import wattWordmark from '../assets/watt-wordmark.png';

/** The app version from package.json. vite.config.ts defines it; a test or the harness may not have it, so it is checked. */
const VERSION = typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : 'unknown';

/**
 * Settings > About: the mark, the wordmark, the app name and the version. Nothing here is editable, so the tab has no draft
 * and no unsaved-changes dot. The two images are decorative; the name is in the text under them.
 */
export function AboutSection() {
  return (
    <div className="about">
      <img className="about-mark" src={wattMark} width={212} height={256} alt="" aria-hidden="true" />
      <img className="about-wordmark" src={wattWordmark} width={480} height={155} alt="" aria-hidden="true" />
      <p className="about-name">Watt Terminal</p>
      <p className="about-version">{`Version ${VERSION}`}</p>
    </div>
  );
}
