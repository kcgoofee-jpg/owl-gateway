import type { Shot } from "../pages/workbench/api";

const DATABASE = "owl-workbench-history";
const STORE = "images";

export type HistoryRecord = {
  id: string;
  createdAt: number;
  bytes: ArrayBuffer;
  ext: string;
  attrs: Shot["attrs"];
  label?: string;
};

let lastCreatedAt = 0;

export function shotToHistoryRecord(shot: Shot): HistoryRecord {
  const binary = atob(shot.b64);
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  lastCreatedAt = Math.max(Date.now(), lastCreatedAt + 1);
  return {
    id: globalThis.crypto?.randomUUID?.() ?? `${lastCreatedAt}-${Math.random().toString(36).slice(2)}`,
    createdAt: lastCreatedAt,
    bytes: bytes.buffer,
    ext: shot.ext,
    attrs: { ...shot.attrs },
    ...(shot.label === undefined ? {} : { label: shot.label }),
  };
}

export function historyRecordToShot(record: HistoryRecord): Shot {
  const bytes = new Uint8Array(record.bytes);
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  }
  const b64 = btoa(binary);
  const mime =
    record.ext === "jpg" || record.ext === "jpeg"
      ? "image/jpeg"
      : record.ext === "webp"
        ? "image/webp"
        : "image/png";
  return {
    b64,
    url: `data:${mime};base64,${b64}`,
    ext: record.ext,
    attrs: { ...record.attrs },
    ...(record.label === undefined ? {} : { label: record.label }),
  };
}

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    if (typeof indexedDB === "undefined") {
      reject(new Error("当前浏览器不支持历史持久化；本次打开期间的图片仍可使用。"));
      return;
    }
    const request = indexedDB.open(DATABASE, 1);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE)) {
        request.result.createObjectStore(STORE, { keyPath: "id" });
      }
    };
    request.onerror = () => reject(request.error ?? new Error("图片历史数据库打开失败。"));
    request.onblocked = () => reject(new Error("图片历史数据库被其他标签页阻止，请关闭旧标签页后重试。"));
    request.onsuccess = () => {
      request.result.onversionchange = () => request.result.close();
      resolve(request.result);
    };
  });
}

async function transact<T>(
  mode: IDBTransactionMode,
  operation: (store: IDBObjectStore, done: (value: T) => void) => void,
): Promise<T> {
  const db = await openDatabase();
  return new Promise((resolve, reject) => {
    const transaction = db.transaction(STORE, mode);
    let result: T;
    transaction.oncomplete = () => {
      db.close();
      resolve(result);
    };
    transaction.onerror = () => {
      db.close();
      reject(transaction.error ?? new Error("图片历史保存失败。"));
    };
    transaction.onabort = () => {
      db.close();
      reject(transaction.error ?? new Error("图片历史操作已中止。"));
    };
    try {
      operation(transaction.objectStore(STORE), (value) => {
        result = value;
      });
    } catch (error) {
      transaction.abort();
      db.close();
      reject(error);
    }
  });
}

let writes: Promise<unknown> = Promise.resolve();
function write(operation: (store: IDBObjectStore) => void): Promise<void> {
  const next = writes
    .catch(() => undefined)
    .then(() =>
      transact<void>("readwrite", (store, done) => {
        operation(store);
        done();
      }),
    );
  writes = next;
  return next;
}

export async function loadHistory(): Promise<Shot[]> {
  await writes.catch(() => undefined);
  const records = await transact<HistoryRecord[]>("readonly", (store, done) => {
    const request = store.getAll();
    request.onsuccess = () => done(request.result as HistoryRecord[]);
  });
  return records.sort((a, b) => b.createdAt - a.createdAt).map(historyRecordToShot);
}

export function appendHistory(shots: Shot[]): Promise<void> {
  const records = shots.map(shotToHistoryRecord);
  return write((store) => {
    for (const record of records) store.put(record);
  });
}

export function clearHistory(): Promise<void> {
  return write((store) => {
    store.clear();
  });
}
