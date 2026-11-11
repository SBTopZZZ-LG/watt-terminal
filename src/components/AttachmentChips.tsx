import { formatSize } from '../lib/attachments';

/** One attachment in the input bar. `bytes` is null while it is still being prepared (then it shows "processing..."). */
export interface ChipItem {
  key: number;
  /** The name as shown (escaped for display). */
  name: string;
  kind: 'image' | 'file';
  bytes: number | null;
  thumb?: string;
  processing: boolean;
}

interface Props {
  items: ChipItem[];
  onRemove: (key: number) => void;
}

/** The attachments above the input field: a thumbnail for an image, a name and size for a file, a note while one is processed. */
export function AttachmentChips({ items, onRemove }: Props) {
  return (
    <div className="bar-attachments">
      {items.map((item) => {
        if (item.processing) {
          return (
            <div key={item.key} className="bar-attach is-processing">
              <span className="bar-attach-name" title={item.name}>
                {item.name}
              </span>
              <span className="bar-attach-size">processing...</span>
            </div>
          );
        }
        const remove = (
          <button type="button" className="bar-attach-remove" aria-label={`Remove ${item.name}`} title={`Remove ${item.name}`} onClick={() => onRemove(item.key)}>
            ✕
          </button>
        );
        if (item.kind === 'image' && item.thumb !== undefined) {
          return (
            <div key={item.key} className="bar-attach is-image">
              <img className="bar-attach-thumb" src={item.thumb} alt={item.name} title={item.name} />
              {remove}
            </div>
          );
        }
        return (
          <div key={item.key} className="bar-attach is-file">
            <span className="bar-attach-name" title={item.name}>
              {item.name}
            </span>
            <span className="bar-attach-size">{item.bytes === null ? '' : formatSize(item.bytes)}</span>
            {remove}
          </div>
        );
      })}
    </div>
  );
}
