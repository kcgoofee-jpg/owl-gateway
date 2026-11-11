/** Discord 用户 ID：17～20 位纯数字。 */
export const isDiscordId = (s: string): boolean => /^\d{17,20}$/.test(s.trim());
