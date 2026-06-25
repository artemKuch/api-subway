export const createDictionary = <T>(): Record<string, T> =>
  Object.fromEntries<T>([]);

export const setDictionaryValue = <T>(
  dictionary: Record<string, T>,
  key: string,
  value: T,
): void => {
  Object.defineProperty(dictionary, key, {
    value,
    configurable: true,
    enumerable: true,
    writable: true,
  });
};

export const getDictionaryValue = <T>(
  dictionary: Record<string, T>,
  key: string,
): T | undefined =>
  Object.hasOwn(dictionary, key) ? dictionary[key] : undefined;
