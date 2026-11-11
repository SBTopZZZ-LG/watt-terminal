// Whether the model the agent will use takes images. The status says which model is the default, but not what it accepts: that
// is Model.vision in the saved settings (set from the provider's model list, see types.ts). Pure, so the rules are tested alone.

import type { Settings } from './types';

/**
 * Whether the saved default model accepts images: true or false as the model list said, or null when that is unknown (no
 * saved default, or the default is not in the saved providers, or the list did not say).
 */
export function defaultModelVision(settings: Settings): boolean | null {
  const ref = settings.default_model;
  if (ref === null || ref === undefined) {
    return null;
  }
  const provider = settings.providers.find((p) => p.id === ref.provider_id);
  const model = provider?.models.find((m) => m.id === ref.model_id);
  return model?.vision ?? null;
}

/**
 * Whether the bar shows the "may not accept images" line: the model is known to take no images and the draft has images. An
 * unknown model (null) and a model that takes images (true) get no line.
 */
export function showsVisionWarning(vision: boolean | null | undefined, images: number): boolean {
  return vision === false && images > 0;
}
