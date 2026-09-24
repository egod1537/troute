import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Callout, Classes, Collapse, Dialog, DialogBody, DialogFooter, Intent, Spinner, Tag, TextArea } from "@blueprintjs/core";
import { fetchTravelTimeMatrix, isTenMinuteDuration, isTenMinuteTime, parseInput, type RouteInput, type RouteProviderName, type TravelMode } from "../../api";
import { JOB_PRESETS, type JobPreset } from "../../placePresets";
import { createSample } from "../../sample";

interface NewJobDialogProps {
  isOpen: boolean; dark: boolean; existingJobIds: ReadonlySet<string>;
  onClose: () => void; onCreate: (request: RouteInput) => void;
}
interface FormLocation {
  key: string; id: string; name: string; placeId: string;
  openTime: string; closeTime: string; stayMinutes: string;
}
interface ValidationResult { valid: boolean; message: string }

function locationFromRequest(location: RouteInput["locations"][number], index: number): FormLocation {
  return { key: `${location.id}-${index}-${Date.now()}`, id: location.id, name: location.name ?? "", placeId: location.place_id, openTime: location.open_time, closeTime: location.close_time, stayMinutes: String(location.stay_minutes) };
}
function blankMatrix(size: number): string[][] {
  return Array.from({ length: size }, (_, row) => Array.from({ length: size }, (_, column) => row === column ? "0" : ""));
}
const matrixFromNumbers = (matrix: number[][]) => matrix.map((row) => row.map(String));
function completeMatrix(matrix: string[][], size: number): number[][] | undefined {
  if (matrix.length !== size || matrix.some((row) => row.length !== size || row.some((value) => value.trim() === ""))) return undefined;
  return matrix.map((row) => row.map(Number));
}

export function NewJobDialog({ isOpen, dark, existingJobIds, onClose, onCreate }: NewJobDialogProps) {
  const [jobId, setJobId] = useState("");
  const [countryCode, setCountryCode] = useState("");
  const [travelMode, setTravelMode] = useState<TravelMode>("TRANSIT");
  const [routeProvider, setRouteProvider] = useState<RouteProviderName | "">("");
  const [locations, setLocations] = useState<FormLocation[]>([]);
  const [matrix, setMatrix] = useState<string[][]>([]);
  const [validation, setValidation] = useState<ValidationResult | null>(null);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [rawOpen, setRawOpen] = useState(true);
  const [rawInput, setRawInput] = useState("");
  const [rawEditing, setRawEditing] = useState(false);
  const [csvOpen, setCsvOpen] = useState(false);
  const [csvInput, setCsvInput] = useState("");
  const [matrixLoading, setMatrixLoading] = useState(false);
  const [pendingPreset, setPendingPreset] = useState<JobPreset | null>(null);
  const matrixRequestSequence = useRef(0);
  const matrixAbortController = useRef<AbortController | null>(null);

  function cancelMatrixRequest() {
    matrixAbortController.current?.abort();
    matrixAbortController.current = null;
    matrixRequestSequence.current += 1;
    setMatrixLoading(false);
  }

  function beginMatrixRequest() {
    cancelMatrixRequest();
    const controller = new AbortController();
    matrixAbortController.current = controller;
    const requestSequence = matrixRequestSequence.current;
    setMatrixLoading(true);
    return { controller, requestSequence };
  }

  function loadRequest(request: RouteInput) {
    cancelMatrixRequest();
    setJobId(request.job_id);
    setCountryCode(request.country_code ?? "");
    setTravelMode(request.travel_mode ?? "TRANSIT");
    setRouteProvider(request.route_provider ?? "");
    setLocations(request.locations.map(locationFromRequest));
    setMatrix(request.travel_time_matrix ? matrixFromNumbers(request.travel_time_matrix) : blankMatrix(request.locations.length));
    setRawInput(JSON.stringify(request, null, 2));
    setPendingPreset(null);
    setRawEditing(false); setValidation(null); setFieldErrors({});
  }
  useEffect(() => {
    if (isOpen) { loadRequest(createSample()); setRawOpen(true); setCsvOpen(false); setCsvInput(""); }
    else cancelMatrixRequest();
  }, [isOpen]);
  useEffect(() => () => matrixAbortController.current?.abort(), []);

  const formRequest = useMemo<RouteInput>(() => {
    const suppliedMatrix = completeMatrix(matrix, locations.length);
    return {
      job_id: jobId,
      locations: locations.map((location) => ({ id: location.id.trim(), name: location.name.trim() || undefined, place_id: location.placeId.trim(), open_time: location.openTime, close_time: location.closeTime, stay_minutes: Number(location.stayMinutes) })),
      // Midnight is the contract's minimum earliest-start bound. The solver still selects the latest feasible start.
      start_time: "00:00",
      country_code: countryCode.trim().toUpperCase() || undefined,
      travel_mode: travelMode,
      route_provider: routeProvider || undefined,
      travel_time_matrix: suppliedMatrix,
      debug: { min_job_duration_ms: 4_000 },
    };
  }, [jobId, countryCode, travelMode, routeProvider, locations, matrix]);
  useEffect(() => { if (!rawEditing) setRawInput(JSON.stringify(formRequest, null, 2)); }, [formRequest, rawEditing]);

  function clearFeedback() {
    cancelMatrixRequest();
    setValidation(null); setFieldErrors({}); setRawEditing(false);
  }
  function updateLocation(index: number, field: keyof Omit<FormLocation, "key">, value: string) {
    setLocations((current) => current.map((location, itemIndex) => itemIndex === index ? { ...location, [field]: value } : location));
    clearFeedback();
  }
  function validateForm(validateMatrix = true): RouteInput | null {
    const errors: Record<string, string> = {};
    const suppliedMatrix = validateMatrix ? completeMatrix(matrix, locations.length) : undefined;
    const hasMatrixInput = matrix.some((row, rowIndex) => row.some((value, columnIndex) => rowIndex !== columnIndex && value.trim() !== ""));
    if (!jobId.trim()) errors.jobId = "Job ID를 입력하세요.";
    if (countryCode.trim() && !/^[A-Za-z]{2}$/.test(countryCode.trim())) errors.countryCode = "국가 코드는 ISO alpha-2 두 글자여야 합니다.";
    if (!["TRANSIT", "DRIVING", "WALKING", "BICYCLING"].includes(travelMode)) errors.travelMode = "지원되는 이동수단을 입력하세요.";
    if (routeProvider && !["google", "kakao-mobility", "kakao-maps", "ekispert", "navitime", "otp"].includes(routeProvider)) errors.routeProvider = "등록된 provider 이름을 입력하세요.";
    if (existingJobIds.has(jobId.trim())) errors.jobId = `job_id "${jobId.trim()}"가 이 세션에 이미 존재합니다.`;
    if (locations.length < 2) errors.locations = "장소가 최소 2개 필요합니다.";
    const ids = new Set<string>();
    locations.forEach((location, index) => {
      const prefix = `location.${index}`;
      if (!location.id.trim()) errors[`${prefix}.id`] = "ID가 필요합니다.";
      else if (ids.has(location.id.trim())) errors[`${prefix}.id`] = "ID는 중복될 수 없습니다.";
      ids.add(location.id.trim());
      if (!location.placeId.trim() && !suppliedMatrix) errors[`${prefix}.placeId`] = "매트릭스가 없으면 tcache 조회용 Place ID가 필요합니다.";
      if (!isTenMinuteTime(location.openTime)) errors[`${prefix}.openTime`] = "Open은 10분 단위 HH:MM이어야 합니다.";
      if (!isTenMinuteTime(location.closeTime)) errors[`${prefix}.closeTime`] = "Close는 10분 단위 HH:MM이어야 합니다.";
      if (isTenMinuteTime(location.openTime) && isTenMinuteTime(location.closeTime) && location.openTime > location.closeTime) errors[`${prefix}.closeTime`] = "Close는 Open보다 빠를 수 없습니다.";
      const stay = Number(location.stayMinutes);
      if (location.stayMinutes.trim() === "" || !isTenMinuteDuration(stay)) errors[`${prefix}.stayMinutes`] = "체류시간은 0 이상의 10분 배수여야 합니다.";
    });
    if (validateMatrix && hasMatrixInput && (matrix.length !== locations.length || matrix.some((row) => row.length !== locations.length))) errors.matrix = "매트릭스 크기가 장소 수와 일치하지 않습니다.";
    else if (validateMatrix && hasMatrixInput) matrix.forEach((row, rowIndex) => row.forEach((value, columnIndex) => {
      const parsed = Number(value);
      if (value.trim() === "" || !Number.isInteger(parsed) || parsed < 0) errors[`matrix.${rowIndex}.${columnIndex}`] = "0 이상의 정수를 입력하세요.";
      else if (rowIndex === columnIndex && parsed !== 0) errors[`matrix.${rowIndex}.${columnIndex}`] = "대각선 값은 0이어야 합니다.";
    }));
    setFieldErrors(errors);
    if (Object.keys(errors).length) { setValidation({ valid: false, message: Object.values(errors)[0] }); return null; }
    return formRequest;
  }
  function parseAndValidate() {
    try {
      const request = rawEditing ? parseInput(rawInput) : validateForm();
      if (!request) return null;
      if (existingJobIds.has(request.job_id)) throw new Error(`job_id "${request.job_id}"가 이 세션에 이미 존재합니다.`);
      setFieldErrors({}); setValidation({ valid: true, message: "유효함" }); return request;
    } catch (error) { setValidation({ valid: false, message: (error as Error).message }); return null; }
  }

  function addLocation() {
    const nextIndex = locations.length;
    const candidate = String.fromCharCode(65 + (nextIndex % 26));
    setLocations((current) => [...current, { key: `new-${Date.now()}-${nextIndex}`, id: current.some((location) => location.id === candidate) ? `L${nextIndex + 1}` : candidate, name: "", placeId: "", openTime: "09:00", closeTime: "18:00", stayMinutes: "0" }]);
    setMatrix((current) => [...current.map((row) => [...row, ""]), Array.from({ length: current.length + 1 }, (_, index) => index === current.length ? "0" : "")]);
    clearFeedback();
  }
  function removeLocation(index: number) {
    setLocations((current) => current.filter((_, itemIndex) => itemIndex !== index));
    setMatrix((current) => current.filter((_, rowIndex) => rowIndex !== index).map((row) => row.filter((_, columnIndex) => columnIndex !== index)));
    clearFeedback();
  }
  function requestPreset(preset: JobPreset) {
    cancelMatrixRequest();
    if (locations.length > 0) setPendingPreset(preset);
    else void applyPreset(preset);
  }
  async function applyPreset(preset: JobPreset) {
    const nextLocations: FormLocation[] = preset.locations.map((location) => ({
      key: `${preset.key}-${location.id}`,
      id: location.id,
      name: location.name,
      placeId: location.placeId,
      openTime: location.openTime,
      closeTime: location.closeTime,
      stayMinutes: String(location.stayMinutes),
    }));
    cancelMatrixRequest();
    setPendingPreset(null);
    setCountryCode(preset.countryCode ?? "");
    setTravelMode(preset.travelMode ?? "WALKING");
    setRouteProvider("");
    setLocations(nextLocations);
    setRawEditing(false);
    setFieldErrors({});
    setCsvOpen(false);
    if (preset.travelTimeMatrix) {
      setMatrix(matrixFromNumbers(preset.travelTimeMatrix));
      setMatrixLoading(false);
      setValidation({ valid: true, message: `${preset.name} preset을 적용했습니다.` });
      return;
    }

    setMatrix(blankMatrix(nextLocations.length));
    setValidation(null);
    const request: RouteInput = {
      job_id: jobId,
      locations: nextLocations.map((location) => ({
        id: location.id,
        name: location.name,
        place_id: location.placeId,
        open_time: location.openTime,
        close_time: location.closeTime,
        stay_minutes: Number(location.stayMinutes),
      })),
      start_time: "00:00",
      country_code: preset.countryCode,
      travel_mode: preset.travelMode ?? "WALKING",
    };
    const { controller, requestSequence } = beginMatrixRequest();
    try {
      const result = await fetchTravelTimeMatrix(request, controller.signal);
      if (
        result.length !== nextLocations.length ||
        result.some((row) => row.length !== nextLocations.length)
      ) throw new Error("tcache 매트릭스 크기가 장소 수와 일치하지 않습니다.");
      if (matrixRequestSequence.current !== requestSequence) return;
      setMatrix(matrixFromNumbers(result));
      setValidation({ valid: true, message: `${preset.name} preset과 매트릭스를 적용했습니다.` });
    } catch (error) {
      if (matrixRequestSequence.current !== requestSequence) return;
      setValidation({ valid: false, message: `${preset.name} 장소는 적용했지만 매트릭스를 가져오지 못했습니다: ${(error as Error).message}` });
    } finally {
      if (matrixRequestSequence.current === requestSequence) {
        matrixAbortController.current = null;
        setMatrixLoading(false);
      }
    }
  }
  function randomizeMatrix() {
    setMatrix(Array.from({ length: locations.length }, (_, row) => Array.from({ length: locations.length }, (_, column) => row === column ? "0" : String((Math.floor(Math.random() * 9) + 1) * 10)))); clearFeedback();
  }
  function resetMatrix() { setMatrix(blankMatrix(locations.length)); clearFeedback(); }
  function applyCsv() {
    const rows = csvInput.trim().split(/\r?\n/).map((row) => row.trim().split(/[\t,; ]+/));
    if (rows.length !== locations.length || rows.some((row) => row.length !== locations.length)) { setValidation({ valid: false, message: `${locations.length}x${locations.length} CSV가 필요합니다.` }); return; }
    setMatrix(rows); setCsvOpen(false); clearFeedback();
  }
  async function loadMatrix() {
    cancelMatrixRequest();
    const request = validateForm(false); if (!request) return;
    const missingIndex = locations.findIndex((location) => !location.placeId.trim());
    if (missingIndex >= 0) { setFieldErrors({ [`location.${missingIndex}.placeId`]: "tcache 조회에는 Place ID가 필요합니다." }); setValidation({ valid: false, message: "모든 장소의 Place ID를 입력하세요." }); return; }
    const { controller, requestSequence } = beginMatrixRequest();
    try {
      const result = await fetchTravelTimeMatrix(request, controller.signal);
      if (result.length !== locations.length || result.some((row) => row.length !== locations.length)) throw new Error("tcache 매트릭스 크기가 장소 수와 일치하지 않습니다.");
      if (matrixRequestSequence.current !== requestSequence) return;
      setMatrix(matrixFromNumbers(result)); setValidation({ valid: true, message: "tcache 매트릭스를 가져왔습니다." });
    } catch (error) {
      if (matrixRequestSequence.current === requestSequence) setValidation({ valid: false, message: (error as Error).message });
    }
    finally { if (matrixRequestSequence.current === requestSequence) { matrixAbortController.current = null; setMatrixLoading(false); } }
  }
  function formatRaw() {
    try { const request = parseInput(rawInput); setRawInput(JSON.stringify(request, null, 2)); setValidation({ valid: true, message: "유효함" }); }
    catch (error) { setValidation({ valid: false, message: (error as Error).message }); }
  }
  function applyRawToForm() {
    try { loadRequest(parseInput(rawInput)); setValidation({ valid: true, message: "Raw JSON을 폼에 반영했습니다." }); }
    catch (error) { setValidation({ valid: false, message: (error as Error).message }); }
  }
  function create() { const request = parseAndValidate(); if (request) onCreate(request); }
  function closeDialog() { cancelMatrixRequest(); onClose(); }

  return <Dialog className="new-job-dialog" isOpen={isOpen} onClose={closeDialog} portalClassName={dark ? Classes.DARK : undefined} title="새 Job" icon="new-object" canEscapeKeyClose>
    <DialogBody className="new-job-dialog-body">
      <section className="new-job-section preset-section" aria-labelledby="preset-heading">
        <h2 id="preset-heading">Preset</h2>
        <div className="preset-buttons">
          {JOB_PRESETS.map((preset) => <Button key={preset.key} onClick={() => requestPreset(preset)}>{preset.name}</Button>)}
        </div>
      </section>
      <section className="new-job-section" aria-labelledby="basic-info-heading">
        <h2 id="basic-info-heading">기본 정보</h2>
        <div className="basic-info-grid">
          <label><span>Job ID</span><input className={`bp6-input ${fieldErrors.jobId ? "field-invalid" : ""}`} aria-invalid={Boolean(fieldErrors.jobId)} value={jobId} onChange={(event) => { setJobId(event.target.value); clearFeedback(); }} />{fieldErrors.jobId && <small>{fieldErrors.jobId}</small>}</label>
          <label><span>Country</span><input className={`bp6-input ${fieldErrors.countryCode ? "field-invalid" : ""}`} aria-invalid={Boolean(fieldErrors.countryCode)} placeholder="JP" maxLength={2} value={countryCode} onChange={(event) => { setCountryCode(event.target.value); clearFeedback(); }} />{fieldErrors.countryCode && <small>{fieldErrors.countryCode}</small>}</label>
          <label><span>Mode</span><input className={`bp6-input ${fieldErrors.travelMode ? "field-invalid" : ""}`} aria-invalid={Boolean(fieldErrors.travelMode)} placeholder="TRANSIT" value={travelMode} onChange={(event) => { setTravelMode(event.target.value.toUpperCase() as TravelMode); clearFeedback(); }} />{fieldErrors.travelMode && <small>{fieldErrors.travelMode}</small>}</label>
          <label><span>Provider override</span><input className={`bp6-input ${fieldErrors.routeProvider ? "field-invalid" : ""}`} aria-invalid={Boolean(fieldErrors.routeProvider)} placeholder="Policy (optional)" value={routeProvider} onChange={(event) => { setRouteProvider(event.target.value.toLowerCase() as RouteProviderName | ""); clearFeedback(); }} />{fieldErrors.routeProvider && <small>{fieldErrors.routeProvider}</small>}</label>
          <div className="readonly-fact"><span>시간 단위</span><strong>10분</strong></div>
        </div>
      </section>

      <section className="new-job-section" aria-labelledby="locations-heading">
        <div className="section-heading"><div><h2 id="locations-heading">장소</h2><p className={Classes.TEXT_MUTED}>locations[0]은 고정 start, 마지막 항목은 고정 destination이며 중간 장소만 재배치됩니다.</p></div></div>
        <div className="form-table-scroll"><table className="location-input-table"><thead><tr><th>순서</th><th>ID</th><th>이름</th><th>Place ID</th><th>Open</th><th>Close</th><th>체류시간(분)</th><th>삭제</th></tr></thead>
          <tbody>{locations.map((location, index) => {
            const role = index === 0 ? "start" : index === locations.length - 1 ? "destination" : String(index + 1);
            const field = (name: string) => fieldErrors[`location.${index}.${name}`];
            return <tr key={location.key}>
              <td><Tag minimal>{role}</Tag></td>
              <td><input aria-label={`${index + 1}번 장소 ID`} aria-invalid={Boolean(field("id"))} className={`bp6-input ${field("id") ? "field-invalid" : ""}`} value={location.id} onChange={(event) => updateLocation(index, "id", event.target.value)} /></td>
              <td><input aria-label={`${index + 1}번 장소 이름`} className="bp6-input" value={location.name} onChange={(event) => updateLocation(index, "name", event.target.value)} /></td>
              <td><input aria-label={`${index + 1}번 장소 Place ID`} aria-invalid={Boolean(field("placeId"))} className={`bp6-input place-id-input ${field("placeId") ? "field-invalid" : ""}`} value={location.placeId} onChange={(event) => updateLocation(index, "placeId", event.target.value)} /></td>
              <td><input aria-label={`${index + 1}번 장소 Open`} aria-invalid={Boolean(field("openTime"))} className={`bp6-input time-input ${field("openTime") ? "field-invalid" : ""}`} type="time" step="600" value={location.openTime} onChange={(event) => updateLocation(index, "openTime", event.target.value)} /></td>
              <td><input aria-label={`${index + 1}번 장소 Close`} aria-invalid={Boolean(field("closeTime"))} className={`bp6-input time-input ${field("closeTime") ? "field-invalid" : ""}`} type="time" step="600" value={location.closeTime} onChange={(event) => updateLocation(index, "closeTime", event.target.value)} /></td>
              <td><input aria-label={`${index + 1}번 장소 체류시간`} aria-invalid={Boolean(field("stayMinutes"))} className={`bp6-input stay-input ${field("stayMinutes") ? "field-invalid" : ""}`} type="number" min="0" step="10" value={location.stayMinutes} onChange={(event) => updateLocation(index, "stayMinutes", event.target.value)} /></td>
              <td><Button aria-label={`${location.id || index + 1} 장소 삭제`} icon="trash" intent={Intent.DANGER} minimal onClick={() => removeLocation(index)} /></td>
            </tr>;
          })}</tbody></table></div>
        <div className="section-actions"><Button icon="plus" onClick={addLocation}>장소 추가</Button></div>
      </section>

      <section className="new-job-section" aria-labelledby="matrix-heading">
        <div className="section-heading"><div><h2 id="matrix-heading">이동 시간 매트릭스</h2><p className={Classes.TEXT_MUTED}>완성된 매트릭스는 최적화 요청에 포함되어 tcache 조회를 생략합니다. 비워 두면 Place ID로 tcache에서 조회합니다.</p></div></div>
        <div className="form-table-scroll matrix-scroll"><table className="matrix-input-table"><thead><tr><th aria-label="출발 및 도착" />{locations.map((location, index) => <th key={location.key}>{location.id || index + 1}</th>)}</tr></thead>
          <tbody>{locations.map((location, rowIndex) => <tr key={location.key}><th>{location.id || rowIndex + 1}</th>{locations.map((column, columnIndex) => { const error = fieldErrors[`matrix.${rowIndex}.${columnIndex}`]; return <td key={column.key}><input aria-label={`${location.id || rowIndex + 1}에서 ${column.id || columnIndex + 1} 이동 시간`} aria-invalid={Boolean(error)} className={`bp6-input ${error ? "field-invalid" : ""}`} type="number" min="0" step="1" disabled={rowIndex === columnIndex} value={matrix[rowIndex]?.[columnIndex] ?? ""} onChange={(event) => { const value = event.target.value; setMatrix((current) => current.map((row, currentRow) => currentRow === rowIndex ? row.map((cell, currentColumn) => currentColumn === columnIndex ? value : cell) : row)); clearFeedback(); }} /></td>; })}</tr>)}</tbody>
        </table></div>
        <div className="section-actions matrix-actions"><Button icon="cloud-download" onClick={() => void loadMatrix()}>{matrixLoading ? "다시 가져오기" : "tcache에서 가져오기"}</Button><Button icon="random" onClick={randomizeMatrix}>랜덤 생성</Button><Button icon="th" onClick={() => setCsvOpen((open) => !open)}>CSV 붙여넣기</Button><Button icon="reset" onClick={resetMatrix}>초기화</Button>{matrixLoading && <span className={Classes.TEXT_MUTED}><Spinner size={14} /> 매트릭스 생성 중</span>}</div>
        <Collapse isOpen={csvOpen}><div className="csv-editor"><TextArea aria-label="매트릭스 CSV" fill placeholder={"0,30,45\n28,0,15\n40,18,0"} value={csvInput} onChange={(event) => setCsvInput(event.target.value)} /><Button intent={Intent.PRIMARY} onClick={applyCsv}>CSV 적용</Button></div></Collapse>
      </section>

      <section className="new-job-section raw-json-section">
        <Button alignText="left" fill icon="code" endIcon={rawOpen ? "chevron-up" : "chevron-down"} variant="minimal" onClick={() => setRawOpen((open) => !open)}>Raw JSON 보기 / 편집</Button>
        <Collapse isOpen={rawOpen}><div className="raw-json-editor"><div className="dialog-editor-heading"><span>최적화 요청 JSON</span><div><Button size="small" variant="minimal" icon="code" onClick={formatRaw}>포맷</Button><Button size="small" variant="minimal" icon="reset" onClick={() => loadRequest(createSample())}>샘플 복원</Button><Button size="small" variant="minimal" icon="changes" onClick={applyRawToForm}>폼에 반영</Button></div></div><TextArea aria-label="요청 JSON" className="json-editor new-job-editor" fill intent={validation?.valid === false ? Intent.DANGER : Intent.NONE} spellCheck={false} autoCapitalize="off" value={rawInput} onChange={(event) => { setRawInput(event.target.value); setRawEditing(true); setValidation(null); }} /></div></Collapse>
      </section>
      <div className="dialog-validation" aria-live="polite">{validation?.valid ? <Tag icon="tick" intent={Intent.SUCCESS} minimal>{validation.message}</Tag> : validation ? <Callout compact intent={Intent.DANGER} role="alert" title="입력 오류">{validation.message}</Callout> : null}</div>
    </DialogBody>
    <DialogFooter actions={<><Button onClick={closeDialog}>취소</Button><Button icon="tick" disabled={matrixLoading} onClick={parseAndValidate}>검증</Button><Button icon="play" intent={Intent.PRIMARY} disabled={matrixLoading} onClick={create}>Job 생성</Button></>} />
    <Alert cancelButtonText="취소" confirmButtonText="교체" intent={Intent.PRIMARY} isOpen={pendingPreset !== null} onCancel={() => setPendingPreset(null)} onConfirm={() => { if (pendingPreset) void applyPreset(pendingPreset); }}>
      <p>현재 입력을 {pendingPreset?.name} preset으로 교체할까요?</p>
    </Alert>
  </Dialog>;
}
