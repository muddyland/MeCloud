import { uniqueName } from './fileTypes.js';

/**
 * Upload orchestration for the Files app: batches of picked files, dropped
 * folder trees, one progress row per file.
 *
 * The transport (bytes, FileNode creation) and the UI (progress rows, toasts,
 * the node list) are passed in, so the rules that matter — unique names within
 * a folder, folders created before anything goes into them, a failed folder
 * stopping its branch instead of flattening it — are tested without a server
 * or a component.
 *
 * @param {object} deps
 * @param {(file: File, opts: object) => Promise<object|null>} deps.uploadFile
 *        upload bytes and create the node; opts carries parentId, name,
 *        onProgress and onWait
 * @param {(name: string, parentId: string|null) => Promise<object|null>} deps.createFolder
 * @param {(parentId: string|null) => Iterable<string>} deps.siblingNames
 *        names already taken in a folder
 * @param {(node: object) => void} [deps.onCreated]   a node now exists
 * @param {object} deps.rows   progress rows: add(row) → id, patch(id, fields), remove(id)
 * @param {(name: string, message: string) => void} [deps.onError]
 * @param {number} [deps.rowLingerMs]  how long a finished row stays visible
 */
export function createUploader({
  uploadFile, createFolder, siblingNames, onCreated = () => {}, rows,
  onError = () => {}, rowLingerMs = 1200,
}) {
  /** One file with its own progress row. @returns {Promise<boolean>} ok */
  async function uploadOne(file, name, parentId) {
    const id = rows.add({ name, progress: 0, error: '', waiting: '', done: false });
    try {
      const created = await uploadFile(file, {
        parentId,
        name,
        onProgress: (p) => rows.patch(id, { progress: p, waiting: '' }),
        onWait: (ms) => rows.patch(id, { waiting: `rate limited — retrying in ${Math.ceil(ms / 1000)}s` }),
      });
      // Null means the server did not echo the new node; the caller reads the
      // real state back afterwards, so nothing needs inventing here.
      if (created) onCreated(created);
      rows.patch(id, { progress: 1, done: true });
      setTimeout(() => rows.remove(id), rowLingerMs);
      return true;
    } catch (e) {
      const message = e?.message ?? 'Upload failed.';
      // The row stays pinned until dismissed: a toast alone is easy to miss.
      rows.patch(id, { error: message, done: true });
      onError(name, message);
      return false;
    }
  }

  /**
   * A flat batch into one folder. Two files with one name get distinct names
   * rather than two identical entries.
   * @returns {Promise<{ failures: number }>}
   */
  async function uploadBatch(files, parentId) {
    const taken = new Set(siblingNames(parentId ?? null));
    let failures = 0;
    for (const file of files) {
      const name = uniqueName(file.name, taken);
      taken.add(name);
      if (!(await uploadOne(file, name, parentId))) failures += 1;
    }
    return { failures };
  }

  /**
   * A dropped tree, depth-first: each folder is created before anything goes
   * into it, so a child never references a parent that does not exist yet.
   * A folder the server does not confirm stops its branch with an error.
   * @returns {Promise<{ failures: number }>}
   */
  async function uploadTree(nodes, parentId) {
    const taken = new Set(siblingNames(parentId ?? null));
    let failures = 0;
    for (const node of nodes) {
      const name = uniqueName(node.name, taken);
      taken.add(name);
      if (node.kind === 'dir') {
        const created = await createFolder(name, parentId);
        if (!created?.id) {
          throw new Error(`Could not confirm the folder "${name}" — its contents were not uploaded.`);
        }
        onCreated(created);
        if (node.children?.length) failures += (await uploadTree(node.children, created.id)).failures;
      } else if (!(await uploadOne(node.file, name, parentId))) {
        failures += 1;
      }
    }
    return { failures };
  }

  return { uploadOne, uploadBatch, uploadTree };
}
