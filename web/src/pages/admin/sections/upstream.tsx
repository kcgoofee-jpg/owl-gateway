/** 上游：令牌池、账号对比、V5 官方余量、Anlas 消耗核对。 */
import { useAdmin } from "../Admin";
import { LoadError, Loading, SectionHead } from "./_kit";
import { Sect } from "./system/forms";
import AccountsCard from "./upstream/accounts";
import AllowanceCard from "./upstream/allowance";
import type { PoolData } from "./upstream/core";
import ReconcileCard from "./upstream/reconcile";
import TokensCard from "./upstream/tokens";
import "./upstream.css";

export default function Upstream() {
  const { data, error, loading } = useAdmin<PoolData>("/admin/api/overview");
  return (
    <Sect>
      <SectionHead title="上游" desc="NovelAI 账号池与 V5 官方余量" />
      {loading && !data && <Loading rows={3} />}
      {!data && error && <LoadError error={error} />}
      {data && <TokensCard pool={data.pool} configured={data.pool_configured} />}
      <AccountsCard />
      <AllowanceCard />
      <ReconcileCard />
    </Sect>
  );
}
