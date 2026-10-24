/** 领 Key 与功能：自助领 Key、全站功能开关。首页的加入按钮跟着「开放自助领取」走。 */
import { useAdmin } from "../Admin";
import { LoadError, Loading, SectionHead } from "./_kit";
import type { Ops } from "./onboarding/core";
import { FeaturesCard } from "./onboarding/features";
import RoleGateCard from "./onboarding/RoleGate";
import RegistrationCard from "./onboarding/registration";
import { Sect } from "./system/forms";

export default function Onboarding() {
  const ops = useAdmin<Ops>("/admin/api/ops");
  return (
    <Sect>
      <SectionHead title="领 Key 与功能" desc="新成员怎么领 Key、默认能用什么，以及全站功能开关" />
      {ops.loading && !ops.data && <Loading rows={4} />}
      {!ops.data && ops.error && <LoadError error={ops.error} />}
      {ops.data && (
        <>
          <RegistrationCard ops={ops.data} />
          <RoleGateCard ops={ops.data} />
          <FeaturesCard ops={ops.data} />
        </>
      )}
    </Sect>
  );
}
