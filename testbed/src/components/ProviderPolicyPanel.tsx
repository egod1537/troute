import { Card, Classes, Intent, Tag } from "@blueprintjs/core";
import type { RouteProviderPolicyDiagnostics } from "../api";

interface ProviderPolicyPanelProps {
  diagnostics: RouteProviderPolicyDiagnostics | null;
  error: string;
}

export function ProviderPolicyPanel({
  diagnostics,
  error,
}: ProviderPolicyPanelProps) {
  return (
    <Card className="provider-policy-panel" elevation={1} compact>
      <div className="provider-policy-heading">
        <div>
          <h2 className={Classes.HEADING}>Provider Policy</h2>
          <span className={Classes.TEXT_MUTED}>
            서버에서 해석한 현재 국가 × 이동수단 정책
          </span>
        </div>
        {diagnostics && (
          <div className="provider-policy-tags">
            <Tag minimal>mode: {diagnostics.routeProviderMode}</Tag>
            <Tag
              minimal
              intent={
                diagnostics.overrideEnabled ? Intent.WARNING : Intent.NONE
              }
            >
              request override: {diagnostics.overrideEnabled ? "on" : "off"}
            </Tag>
          </div>
        )}
      </div>
      {error ? (
        <p className="provider-policy-error">{error}</p>
      ) : diagnostics ? (
        <div className="table-scroll provider-policy-table-wrap">
          <table
            className={`${Classes.HTML_TABLE} ${Classes.HTML_TABLE_BORDERED} ${Classes.HTML_TABLE_STRIPED}`}
          >
            <thead>
              <tr>
                <th>Country</th>
                <th>Mode</th>
                <th>Provider</th>
                <th>Available</th>
              </tr>
            </thead>
            <tbody>
              {diagnostics.routes.map((route, index) => (
                <tr
                  key={`${route.country ?? "default"}-${route.mode ?? "default"}-${route.provider}-${index}`}
                  title={route.reason}
                >
                  <td>{route.country ?? "default"}</td>
                  <td>{route.mode ?? "default"}</td>
                  <td>{route.provider}</td>
                  <td>
                    <Tag
                      minimal
                      intent={route.available ? Intent.SUCCESS : Intent.DANGER}
                    >
                      {route.available ? "Yes" : "No"}
                    </Tag>
                    {!route.available && route.reason
                      ? ` ${route.reason}`
                      : ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <p className={Classes.TEXT_MUTED}>Provider policy를 불러오는 중…</p>
      )}
    </Card>
  );
}
