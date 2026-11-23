import { formatSize } from '../lib/attachments';
import { useImageViewer } from './ImageViewer';

/** One attachment in the input bar. `bytes` is null while it is still being prepared (then it shows "processing..."). */
export interface ChipItem {
  key: number;
  /** The name as shown (escaped for display). */
  name: string;
  kind: 'image' | 'file';
  bytes: number | null;
  thumb?: string;
  /** An image's full processed bytes, base64, with their mime type: the viewer shows them without a fetch. */
  data?: { mime: string; b64: string };
  processing: boolean;
}

interface Props {
  items: ChipItem[];
  onRemove: (key: number) => void;
}

/** The attachments above the input field: a thumbnail for an image, a name and size for a file, a note while one is processed. */
export function AttachmentChips({ items, onRemove }: Props) {
  const openImage = useImageViewer();
  return (
    <div className="bar-attachments">
      {items.map((item) => {
        if (item.processing) {
          return (
            <div key={item.key} className="bar-attach is-processing">
              <span className="bar-attach-name" data-tip={item.name} data-tip-clip="">
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
              <button
                type="button"
                className="bar-attach-open"
                aria-label={`View ${item.name}`}
                onClick={() =>
                  openImage({ name: item.name, thumb: item.thumb, full: item.data && `data:${item.data.mime};base64,${item.data.b64}` })
                }
              >
                <img className="bar-attach-thumb" src={item.thumb} alt={item.name} title={item.name} draggable={false} />
              </button>
              {remove}
            </div>
          );
        }
        return (
          <div key={item.key} className="bar-attach is-file">
            <span className="bar-attach-name" data-tip={item.name} data-tip-clip="">
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
