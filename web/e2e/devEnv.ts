/**
 * 端到端依赖的本地开发环境：`cargo xtask dev` 在数据目录里写 cookies.json——后台密码与会话、成员会话与成员 Key
 * （全是合成数据）。默认读仓库根目录的 target/owl-dev/cookies.json；`cargo xtask dev --data <目录>` 换了位置时，
 * 用 E2E_COOKIES 指过去。没有这个文件时，需要成员会话的用例自动跳过。
 */
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

export const COOKIES_FILE =
  process.env.E2E_COOKIES ?? resolve(import.meta.dirname, "../../target/owl-dev/cookies.json");

export type DevCreds = {
  admin?: string;
  admin_password?: string;
  member?: string;
  member_key?: string;
  member_discord_id?: string;
};

export function readCreds(): DevCreds {
  return existsSync(COOKIES_FILE) ? (JSON.parse(readFileSync(COOKIES_FILE, "utf8")) as DevCreds) : {};
}
