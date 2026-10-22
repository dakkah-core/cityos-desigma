/** Execute required fixture tests in the exact compiled Windows test artifact. */
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {execFileSync,spawnSync} from 'node:child_process';
import {readFileSync,writeFileSync,mkdirSync,mkdtempSync,realpathSync,lstatSync,copyFileSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';

export const requiredCases=Object.freeze([
  'design_workspace::tests::packaged_registered_specimens_pass_native_identity_checks_for_both_locales',
  'design_workspace::tests::canonical_accordion_native_save_reopen_and_boolean_export',
  'design_workspace::tests::selected_compositions_survive_native_save_reopen_and_zero_edit_export',
  'design_workspace::tests::canonical_frame_native_save_reopen_and_semantic_export',
  'cityos_catalog::tests::captured_repository_themes_create_editable_libraries_and_survive_save_reopen',
]);
export function passedCase(name,result){
  const log=(result.stdout??'')+(result.stderr??'');
  return result.status===0&&!result.error&&log.includes(`test ${name} ... ok`)
    &&/test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured;/u.test(log);
}
export function passedApplicationJourney(result,expectedBase){
  try{
    if(result.status!==0||result.error)return false;
    const report=JSON.parse(result.stdout);
    assert.equal(report.nativeRoundtrip,true);assert.equal(report.nativeApplicationHost,true);assert.equal(report.checkpointHistoryRestored,true);
    assert.equal(report.unchangedOperationCount,0);assert.equal(report.editedOperationCount,1);assert.equal(report.undoOperationCount,0);
    assert.equal(report.embeddedRendererObserved,false);assert.deepEqual(report.canonicalBase,expectedBase);return true;
  }catch{return false;}
}
const hash=file=>createHash('sha256').update(readFileSync(file)).digest('hex');
function physicalFile(file){assert.ok(lstatSync(file).isFile()&&!lstatSync(file).isSymbolicLink());return realpathSync(file);}
function readJson(file){assert.ok(lstatSync(file).size<=4*1024*1024);return JSON.parse(readFileSync(physicalFile(file),'utf8'));}

export async function qualify({cmsRoot,fixtureRoot,binary,binarySha256,appBinary,appSha256}){
  const root=fileURLToPath(new URL('../../',import.meta.url));
  cmsRoot=realpathSync(cmsRoot);fixtureRoot=realpathSync(fixtureRoot);binary=physicalFile(binary);
  const relative=path.relative(realpathSync(path.join(root,'target')),binary);
  assert.ok(relative&&!relative.startsWith('..')&&!path.isAbsolute(relative),'Test binary must belong to this fork target directory');
  assert.match(binarySha256,/^[a-f0-9]{64}$/u);assert.equal(hash(binary),binarySha256,'Selected native artifact changed');
  appBinary=physicalFile(appBinary);const appRelative=path.relative(realpathSync(path.join(root,'target')),appBinary);
  assert.ok(appRelative&&!appRelative.startsWith('..')&&!path.isAbsolute(appRelative));
  assert.match(appSha256,/^[a-f0-9]{64}$/u);assert.equal(hash(appBinary),appSha256,'Selected application artifact changed');
  const catalog=readJson(path.join(cmsRoot,'config/toolchains/node-typescript.json'));
  assert.equal(process.versions.node,catalog.node,'Select the consuming CMS canonical Node runtime');
  const {acquireWorkspaceValidationLease}=await import(pathToFileURL(path.join(cmsRoot,'scripts/toolchains/workspace-validation-lease.mjs')).href);
  const release=acquireWorkspaceValidationLease({timeoutMs:55000});
  try{
    const reader=path.join(fixtureRoot,'reader'),evidence=path.join(fixtureRoot,'evidence'),themes=path.join(fixtureRoot,'theme-fixtures');
    const inputs=[path.join(reader,'package.json'),path.join(reader,'workspace-package.json'),path.join(evidence,'smoke.json'),path.join(themes,'fixtures.json'),
      physicalFile(path.join(evidence,'canonical-bundle.json'))];
    const readerPackage=readJson(inputs[0]),workspace=readJson(inputs[1]),smoke=readJson(inputs[2]),themeFixture=readJson(inputs[3]);
    assert.equal(readerPackage.schemaVersion,'cityos.design-catalog.package.v1');
    assert.equal(workspace.schemaVersion,'cityos.design-workspace.package.v1');
    assert.equal(smoke.schemaVersion,'cityos.design-workspace.smoke.v1');
    assert.deepEqual(readJson(inputs[4]).draft.base,smoke.canonicalBase,'Native bundle must match the recorded canonical base');
    assert.equal(themeFixture.snapshot.canonicalCommit,smoke.canonicalBase.commit,'Fixtures must observe the same canonical commit');
    assert.ok(themeFixture.themes.length>1&&themeFixture.themes.length<=64);
    for(const [ref,expected]of Object.entries(readerPackage.checksums)){
      assert.ok(ref&&!ref.includes('..')&&!ref.includes('\\')&&!path.isAbsolute(ref)&&!ref.includes(':'));
      const file=physicalFile(path.join(reader,ref));assert.equal(hash(file),expected,'Packaged reader integrity: '+ref);inputs.push(file);
    }
    for(const [ref,expected]of Object.entries(workspace.assets)){
      assert.ok(['preview.js','preview.css'].includes(ref));const file=physicalFile(path.join(reader,ref));assert.equal(hash(file),expected);inputs.push(file);
    }
    const identities=Object.fromEntries([...new Set(inputs)].map(file=>[file,hash(file)]));
    const parent=path.join(root,'.build/upstream');mkdirSync(parent,{recursive:true});const output=mkdtempSync(path.join(parent,'native-fixtures-'));
    const git=args=>execFileSync('git',['-C',root,...args],{windowsHide:true,encoding:'utf8'}).trim();
    const observeSource=()=>{
      const sourceFiles=git(['ls-files','-z','-c','-o','--exclude-standard','--','crates','Cargo.toml','Cargo.lock','rust-toolchain.toml','scripts/upstream/verify-native-candidate.mjs'])
        .split('\0').filter(Boolean).sort().map(ref=>[ref,hash(physicalFile(path.join(root,ref)))]);
      const status=git(['status','--porcelain']);
      return {commit:git(['rev-parse','HEAD']),dirty:!!status,sourceDigest:createHash('sha256').update(JSON.stringify(sourceFiles)).digest('hex'),
        statusDigest:createHash('sha256').update(status).digest('hex')};
    };
    const sourceObservation=observeSource();
    const env={...process.env,CITYOS_DESIGN_WORKSPACE_PACKAGE:reader,CITYOS_DESIGN_WORKSPACE_REPOSITORY:cmsRoot,
      CITYOS_DESIGN_WORKSPACE_EVIDENCE:evidence,CITYOS_DESIGN_WORKSPACE_NODE:process.execPath,CITYOS_NATIVE_LIBRARY_EVIDENCE:themes};
    const inventory=spawnSync(binary,['--list'],{cwd:root,env,windowsHide:true,encoding:'utf8',timeout:30000});
    assert.equal(inventory.status,0,'Native test inventory unavailable');
    for(const name of requiredCases)assert.equal(inventory.stdout.split(/\r?\n/u).filter(line=>line===name+': test').length,1,'Required native case absent: '+name);
    const results=[];
    for(const [index,name]of requiredCases.entries()){
      const result=spawnSync(binary,['--ignored','--exact',name,'--test-threads=1','--nocapture'],{cwd:root,env,windowsHide:true,encoding:'utf8',timeout:600000,maxBuffer:12*1024*1024});
      const log=path.join(output,`case-${index+1}.log`);writeFileSync(log,(result.stdout??'')+(result.stderr??''));
      results.push({name,exitCode:result.status,success:passedCase(name,result),logSha256:hash(log)});
    }
    const applicationOutput=path.join(output,'application');mkdirSync(applicationOutput);
    copyFileSync(path.join(evidence,'canonical-bundle.json'),path.join(applicationOutput,'canonical-bundle.json'));
    const application=spawnSync(appBinary,['--cityos-verify-workspace',applicationOutput,'--cityos-repository',cmsRoot,
      '--cityos-reader',path.join(reader,'reader.mjs'),'--cityos-node',process.execPath,'--cityos-cache',path.join(evidence,'canonical-cache')],
      {cwd:root,env,windowsHide:true,encoding:'utf8',timeout:300000,maxBuffer:8*1024*1024});
    const applicationLog=path.join(output,'application.log');writeFileSync(applicationLog,(application.stdout??'')+(application.stderr??''));
    results.push({name:'native-application-checkpoint-roundtrip',exitCode:application.status,
      success:passedApplicationJourney(application,smoke.canonicalBase),logSha256:hash(applicationLog)});
    for(const [file,expected]of Object.entries(identities))assert.equal(hash(file),expected,'Qualification input changed: '+file);
    assert.equal(hash(binary),binarySha256);assert.equal(hash(appBinary),appSha256);assert.deepEqual(observeSource(),sourceObservation,'Fork source changed during qualification');
    const receipt={schemaVersion:'cityos.desigma.native-fixture-check.v3',sourceObservation,binarySha256,appSha256,inputs:identities,results,
      passed:results.filter(row=>row.success).length,failed:results.filter(row=>!row.success).length,skipped:0,
      success:results.every(row=>row.success),artifactBuildProvenanceQualified:false,installedGuiQualified:false,embeddedWebViewQualified:false,activationQualified:false,
      nonClaims:['Observed source HEAD is not an attestation of the earlier compilation. The artifact build receipt remains a separate gate.']};
    writeFileSync(path.join(output,'receipt.json'),JSON.stringify(receipt,null,2)+'\n');return {output,...receipt};
  }finally{release();}
}
if(process.argv[1]&&import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href){
  assert.equal(process.argv.length,8,'Usage: node verify-native-candidate.mjs <cms-root> <fixture-root> <test-binary> <test-sha256> <app-binary> <app-sha256>');
  const result=await qualify({cmsRoot:process.argv[2],fixtureRoot:process.argv[3],binary:process.argv[4],binarySha256:process.argv[5],appBinary:process.argv[6],appSha256:process.argv[7]});
  console.log(JSON.stringify({output:result.output,passed:result.passed,failed:result.failed,skipped:result.skipped,success:result.success}));
  if(!result.success)process.exitCode=1;
}
