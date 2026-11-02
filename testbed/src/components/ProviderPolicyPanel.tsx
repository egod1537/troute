import {
  Button,
  Classes,
  Dialog,
  DialogBody,
  DialogFooter,
  Intent,
  Spinner,
  Tag,
} from "@blueprintjs/core";
import type { RouteProviderPolicyDiagnostics } from "../api";

interface ProviderPolicyPanelProps {
  diagnostics: RouteProviderPolicyDiagnostics | null;
  error: string;
  isOpen: boolean;
  dark: boolean;
  loading: boolean;
  onClose: () => void;
  onRefresh: () => void;
}

export function ProviderPolicyPanel({
  diagnostics,
  error,
  isOpen,
  dark,
  loading,
  onClose,
  onRefresh,
}: ProviderPolicyPanelProps) {
  return (
    <Dialog
      className="provider-policy-dialog"
      isOpen={isOpen}
      onClose={onClose}
      portalClassName={dark ? Classes.DARK : undefined}
      title="Provider Policy"
      icon="settings"
    >
      <DialogBody>
        <div className="provider-policy-heading">
          <p className={Classes.TEXT_MUTED}>
            서버에서 해석한 현재 국가 × 이동수단 라우팅 정책입니다.
          </p>
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
          <p className="provider-policy-error" role="alert">{error}</p>
        ) : diagnostics ? (
          <div className="table-scroll provider-policy-table-wrap">
            <table
              aria-label="Provider 라우팅 정책"
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
          <div className="provider-policy-loading" aria-live="polite">
            {loading && <Spinner size={18} />}
            <span className={Classes.TEXT_MUTED}>
              {loading ? "Provider policy를 불러오는 중…" : "불러온 정책이 없습니다."}
            </span>
          </div>
        )}
      </DialogBody>
      <DialogFooter
        actions={
          <>
            <Button icon="refresh" loading={loading} onClick={onRefresh}>
              새로고침
            </Button>
            <Button intent={Intent.PRIMARY} onClick={onClose}>닫기</Button>
          </>
        }
      />
    </Dialog>
  );
}
