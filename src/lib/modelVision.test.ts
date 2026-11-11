import { describe, expect, it } from 'vitest';
import { defaultModelVision, showsVisionWarning } from './modelVision';
import type { Model, Provider, Settings } from './types';

const model = (id: string, vision?: boolean | null): Model => ({
  id,
  context_limit: null,
  provider_context: null,
  source: 'fetched',
  ...(vision === undefined ? {} : { vision }),
});

const provider = (id: string, models: Model[]): Provider => ({
  id,
  name: id,
  chat_url: 'https://example.test/v1/chat/completions',
  models_url: null,
  headers: [],
  stream: true,
  max_tokens: null,
  models,
});

const settings = (providers: Provider[], default_model: Settings['default_model']): Settings =>
  ({ version: 1, providers, default_model, global_context_limit: null }) as unknown as Settings;

describe('defaultModelVision', () => {
  it('is the vision flag of the saved default model', () => {
    const s = settings([provider('acme', [model('big', true), model('small', false)])], { provider_id: 'acme', model_id: 'small' });
    expect(defaultModelVision(s)).toBe(false);
    expect(defaultModelVision(settings([provider('acme', [model('big', true)])], { provider_id: 'acme', model_id: 'big' }))).toBe(true);
  });

  it('is null when there is no saved default', () => {
    expect(defaultModelVision(settings([provider('acme', [model('big', false)])], null))).toBeNull();
  });

  it('is null when the default is not in the saved providers or models', () => {
    expect(defaultModelVision(settings([provider('acme', [model('big', false)])], { provider_id: 'other', model_id: 'big' }))).toBeNull();
    expect(defaultModelVision(settings([provider('acme', [model('big', false)])], { provider_id: 'acme', model_id: 'gone' }))).toBeNull();
  });

  it('is null when the list did not say (the flag is absent or null)', () => {
    expect(defaultModelVision(settings([provider('acme', [model('big')])], { provider_id: 'acme', model_id: 'big' }))).toBeNull();
    expect(defaultModelVision(settings([provider('acme', [model('big', null)])], { provider_id: 'acme', model_id: 'big' }))).toBeNull();
  });
});

describe('showsVisionWarning', () => {
  it('shows only when the model is known to take no images and the draft has images', () => {
    expect(showsVisionWarning(false, 1)).toBe(true);
    expect(showsVisionWarning(false, 3)).toBe(true);
  });

  it('does not show for a model that takes images, an unknown model, or a draft with no images', () => {
    expect(showsVisionWarning(true, 2)).toBe(false);
    expect(showsVisionWarning(null, 2)).toBe(false);
    expect(showsVisionWarning(undefined, 2)).toBe(false);
    expect(showsVisionWarning(false, 0)).toBe(false);
  });
});
