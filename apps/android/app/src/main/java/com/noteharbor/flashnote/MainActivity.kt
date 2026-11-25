package com.noteharbor.flashnote

import android.Manifest
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import androidx.core.content.ContextCompat
import androidx.compose.foundation.background
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.sp
import androidx.compose.ui.graphics.SolidColor
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.compose.setContent
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import org.json.JSONObject
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.coroutines.cancellation.CancellationException

class MainActivity : ComponentActivity() {
 override fun onCreate(savedInstanceState: Bundle?) {
  super.onCreate(savedInstanceState)
  setContent {
   PaperTheme {
    val preview=BuildConfig.DEBUG && intent.getBooleanExtra("design_preview",false)
    val model: NotesModel=viewModel(factory=viewModelFactory {initializer {NotesModel(application,preview)}})
    NotesApp(model,if(preview) intent.getStringExtra("preview_screen") ?: "notes" else if(intent.getBooleanExtra("open_account",false)) "me" else "notes")
   }
  }
 }
}
private const val DEFAULT_GROUP="00000000-0000-0000-0000-000000000000"

@OptIn(ExperimentalMaterial3Api::class)
@Composable fun NotesApp(model: NotesModel,initialScreen: String="notes") {
 val context = LocalContext.current
 val lifecycle = LocalLifecycleOwner.current.lifecycle
 var foreground by remember { mutableStateOf(false) }
 DisposableEffect(lifecycle, model) {
  val observer = LifecycleEventObserver { _, event ->
   if (event == Lifecycle.Event.ON_RESUME) { foreground = true; model.foregroundEntered() }
   if (event == Lifecycle.Event.ON_PAUSE) { foreground = false; model.stopRecording() }
  }
  lifecycle.addObserver(observer)
  onDispose { lifecycle.removeObserver(observer); model.stopRecording() }
 }
 val scope = rememberCoroutineScope()
 var startAfterPermission by remember { mutableStateOf(false) }
 val requestMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
  startAfterPermission = granted
  if (!granted) model.error = "允许使用麦克风后即可录音"
 }
 LaunchedEffect(foreground, startAfterPermission) {
  if (foreground && startAfterPermission) { startAfterPermission = false; model.startRecording() }
 }
 var attachTo by remember { mutableStateOf<Note?>(null) }
 val picker=rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
  if(uri!=null) attachTo?.let {model.attach(uri,it)}
  attachTo=null
 }
 var query by rememberSaveable { mutableStateOf("") }
 var group by rememberSaveable { mutableStateOf("") }
 var trash by rememberSaveable { mutableStateOf(false) }
 var page by rememberSaveable { mutableStateOf(if(initialScreen in listOf("groups","me")) initialScreen else "notes") }
 var settings by rememberSaveable { mutableStateOf(initialScreen=="me") }
 var editing by rememberSaveable { mutableStateOf(false) }
 var editId by rememberSaveable { mutableStateOf("") }
 var revision by rememberSaveable { mutableStateOf("") }
 var original by rememberSaveable { mutableStateOf("") }
 var title by rememberSaveable { mutableStateOf("") }
 var body by rememberSaveable(stateSaver = TextFieldValue.Saver) { mutableStateOf(TextFieldValue()) }
 val source = body.text
 var editGroup by rememberSaveable { mutableStateOf(DEFAULT_GROUP) }
 var showGroup by remember { mutableStateOf(false) }
 var groupName by remember { mutableStateOf("") }
 var groupColor by remember {mutableStateOf(0x245356L)}
 var confirmLeave by remember { mutableStateOf(false) }
 var revisions by remember { mutableStateOf<List<NoteRevision>?>(null) }
 var selectedRevision by remember { mutableStateOf<NoteRevision?>(null) }
 var transcribing by remember { mutableStateOf<String?>(null) }
 val audioPlayer = remember { EditorAudioPlayer() }
 val availableTranscription = remember(context) { canTranscribe(context) }
 DisposableEffect(audioPlayer) { onDispose { audioPlayer.stop() } }
 LaunchedEffect(foreground, editing) { if (!foreground || !editing) audioPlayer.stop() }
 val recordingActive = model.recording || model.recordingToken != null
 val editorBusy = model.busy || recordingActive || transcribing != null
 val dirty=editing && original != title+"\u0000"+source+"\u0000"+editGroup
 fun open(n: Note?) {
  editId=n?.id ?: ""; revision=n?.revision ?: ""; title=n?.title ?: ""; body=TextFieldValue(n?.source ?: "")
  editGroup=n?.group ?: group.ifEmpty { DEFAULT_GROUP }; original=title+"\u0000"+body.text+"\u0000"+editGroup; editing=true
 }
 LaunchedEffect(foreground, editing, model.account) {
  if (foreground && !editing && model.account != null) {
   while (true) {
    model.sync(automatic = true)
    delay(4_000)
   }
  }
 }
 var previewOpened by rememberSaveable {mutableStateOf(false)}
 LaunchedEffect(initialScreen,model.notes) {
  if(initialScreen=="editor" && !previewOpened && model.notes.isNotEmpty()) {previewOpened=true;open(model.notes.first())}
 }
 fun close() { if(editorBusy) return; if(dirty) confirmLeave=true else editing=false }
 fun save() {
  if (editorBusy) return
  val baseline=if(editId.isEmpty()) null else Note(editId,revision,title,editGroup,source,false,0,null)
  model.save(baseline,title,source,editGroup) { editing=false; model.message="已保存到本机" }
 }
 fun commitRecording(copy: Boolean = false) {
  val token = model.recordingToken ?: return
  val savedTitle = title; val savedSource = source; val savedGroup = editGroup
  model.saveRecording(if (copy) "" else editId, revision, savedTitle, savedSource, savedGroup, token) {
   model.lastSavedNote?.let { note ->
    editId = note.id; revision = note.revision
    if (body.text == savedSource) body = TextFieldValue(note.source, TextRange(body.selection.end.coerceIn(0, note.source.length)))
    original = savedTitle + "\u0000" + (model.lastSavedNote?.source ?: savedSource) + "\u0000" + savedGroup
   }
  }
 }
 LaunchedEffect(editing, model.recordingToken) {
  if (editing && !model.busy && model.error == null && model.recordingToken != null) commitRecording()
 }
 LaunchedEffect(editId, model.notes) {
  if (editing && !dirty && !recordingActive && editId.isNotEmpty()) {
   val updated = model.notes.find { it.id == editId && !it.deleted }
   if (updated != null && updated.revision != revision) {
    revision = updated.revision; title = updated.title; editGroup = updated.group
    body = TextFieldValue(updated.source, TextRange(body.selection.end.coerceIn(0, updated.source.length)))
    original = title + "\u0000" + updated.source + "\u0000" + editGroup
   }
  }
 }
 LaunchedEffect(editing,title,source,editGroup,editorBusy) {
  if(editing && dirty && !editorBusy && model.error==null && (title.isNotBlank() || source.isNotBlank())) {
   delay(1200)
   val savedTitle=title;val savedSource=source;val savedGroup=editGroup
   val baseline=if(editId.isEmpty()) null else Note(editId,revision,title,editGroup,source,false,0,null)
   model.save(baseline,savedTitle,savedSource,savedGroup) {
    model.lastSavedNote?.let { n -> editId=n.id;revision=n.revision }
    original=savedTitle+"\u0000"+savedSource+"\u0000"+savedGroup
   }
  }
 }
 BackHandler(enabled=editing || page!="notes" || trash) { if(editing) close() else {settings=false;page="notes";trash=false} }
 Scaffold(
  containerColor=Paper,
  topBar={
   Column(Modifier.statusBarsPadding()) {
    Row(Modifier.fillMaxWidth().height(62.dp).padding(horizontal=20.dp),verticalAlignment=Alignment.CenterVertically) {
     if(editing) {
      IconButton(enabled=!model.busy,onClick={close()}) {LineIcon("back","返回")}
      var chooseGroup by remember {mutableStateOf(false)}
      Box {
       Surface(onClick={chooseGroup=true},shape=RoundedCornerShape(6.dp),border=BorderStroke(1.dp,Edge),color=CardPaper) {
        Row(Modifier.padding(horizontal=10.dp,vertical=7.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(6.dp)) {LineIcon("folder",modifier=Modifier.size(15.dp),color=Color((model.groups.find {it.id==editGroup}?.color ?: 0x245356) or 0xFF000000));Text(model.groups.find {it.id==editGroup}?.name ?: "默认",fontSize=13.sp);LineIcon("down",modifier=Modifier.size(12.dp))}
       }
       DropdownMenu(expanded=chooseGroup,onDismissRequest={chooseGroup=false}) {model.groups.forEach {g -> DropdownMenuItem(text={Text(g.name)},onClick={editGroup=g.id;chooseGroup=false})}}
      }
      Spacer(Modifier.weight(1f))
      Row(verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(4.dp)) {if(!dirty && !model.busy) LineIcon("tick",modifier=Modifier.size(14.dp));Text(if(model.busy) "保存中…" else if(dirty) "未保存" else "已保存",fontSize=11.sp,color=Muted)}
      TextButton(enabled=!editorBusy,onClick={save()}) {Text("完成")}
     } else {
      Text(if(settings) "我的" else if(page=="groups") "分组" else if(trash) "回收站" else "索笺",fontFamily=FontFamily.SansSerif,fontWeight=FontWeight.Bold,fontSize=26.sp,color=Ink)
      Spacer(Modifier.weight(1f))
      if(page=="groups" && !settings) TextButton(onClick={showGroup=true}) {LineIcon("plus",modifier=Modifier.size(17.dp));Text(" 新建分组")}
      else if(!settings) {
       IconButton(enabled=!model.busy,onClick={if(model.account!=null) model.sync() else {settings=true;page="me"}},modifier=Modifier.size(44.dp)) {
        Box(contentAlignment=Alignment.Center) {
         LineIcon(if(model.synced) "cloud-check" else "cloud",when {model.syncing->"正在同步";model.account==null->"未登录，点击登录";model.syncFailed->"同步失败，点击重试";model.synced->"已同步，点击再次同步";else->"已登录，点击同步"},modifier=Modifier.size(23.dp),color=if(model.syncFailed) Color(0xFFB4473B) else if(model.account==null) Muted else Ink)
         if(model.syncing) CircularProgressIndicator(Modifier.size(32.dp),color=Ink,strokeWidth=1.dp)
        }
       }
      }
     }
    }
    if(model.busy) LinearProgressIndicator(Modifier.fillMaxWidth().height(2.dp),color=Ink)
   }
  },
  bottomBar={ if(!editing) {
   Column {HorizontalDivider(color=Edge)
    Row(Modifier.fillMaxWidth().background(Paper).navigationBarsPadding().height(56.dp),horizontalArrangement=Arrangement.SpaceEvenly) {
     listOf("notes" to "便签","groups" to "分组","me" to "我的").forEach { (key,label) ->
      val selected=page==key
      Column(Modifier.weight(1f).fillMaxHeight().clickable {page=key;settings=key=="me";trash=false}.padding(top=7.dp),horizontalAlignment=Alignment.CenterHorizontally,verticalArrangement=Arrangement.spacedBy(3.dp)) {
       LineIcon(when(key){"notes"->"note";"groups"->"folder";else->"user"},label,color=if(selected) Ink else Muted,modifier=Modifier.size(21.dp))
       Text(label,fontSize=12.sp,color=if(selected) Ink else Muted,fontWeight=if(selected) FontWeight.SemiBold else FontWeight.Normal)
      }
     }
    }
   }
  }},
  floatingActionButton={if(!editing && !settings && page=="notes" && !trash) FloatingActionButton(onClick={open(null)},modifier=Modifier.size(52.dp),shape=CircleShape,containerColor=Ink,contentColor=CardPaper,elevation=FloatingActionButtonDefaults.elevation(defaultElevation=2.dp,pressedElevation=3.dp)) {LineIcon("plus","新建便签",Modifier.size(27.dp),CardPaper)}}
 ) { padding ->
  Column(Modifier.fillMaxSize().padding(padding).imePadding().padding(bottom=if(!editing && !settings && page=="notes" && !trash) 68.dp else 0.dp)) {
   if(editing) {
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal=24.dp)) {
     Spacer(Modifier.height(12.dp))
     BasicTextField(title,{title=it},textStyle=TextStyle(fontSize=25.sp,fontWeight=FontWeight.Bold,color=TextInk),cursorBrush=SolidColor(Ink),modifier=Modifier.fillMaxWidth(),decorationBox={inner -> if(title.isEmpty()) Text("无标题便签",fontSize=25.sp,color=Muted);inner()})
     Row(Modifier.padding(top=12.dp,bottom=20.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(6.dp)) {LineIcon("calendar",modifier=Modifier.size(15.dp),color=Muted);Text(SimpleDateFormat("yyyy年M月d日  EEEE",Locale.CHINA).format(Date(model.notes.find {it.id==editId}?.created ?: System.currentTimeMillis())),fontSize=12.sp,color=Muted)}
     BasicTextField(body,{body=it},visualTransformation=remember {PaperMarkdownTransformation()},textStyle=TextStyle(fontSize=16.sp,lineHeight=27.sp,color=TextInk),cursorBrush=SolidColor(Ink),modifier=Modifier.fillMaxWidth().heightIn(min=140.dp),readOnly=recordingActive,decorationBox={inner -> if(source.isEmpty()) Text("写下此刻的想法…",color=Muted,fontSize=17.sp);inner()})
     val editedNote = model.notes.find { it.id == editId }
     editedNote?.attachments?.forEach {a ->
      if(a.isImage) NoteThumbnail(a,root=model.assetRoot,modifier=Modifier.fillMaxWidth().height(180.dp)) {model.openAttachment(a)}
      else if (a.audioBlock != null) {
       Row(verticalAlignment = Alignment.CenterVertically) {
        val playing = audioPlayer.playing == a.asset
        TextButton(enabled = !editorBusy, onClick = {
         runCatching { audioPlayer.toggle(model.assetRoot, a.asset) }.onFailure { model.error = "无法播放录音" }
        }) { LineIcon(if (playing) "stop" else "play"); Text(if (playing) "  停止播放" else "  播放录音") }
        val seconds = a.durationMs / 1000
        Text("%d:%02d".format(seconds / 60, seconds % 60), fontSize = 12.sp, color = Muted)
        if (a.transcript == null && availableTranscription) TextButton(enabled = !editorBusy, onClick = {
         audioPlayer.stop()
         transcribing = a.asset
         scope.launch {
          try {
           val text = transcribe(context, model.assetRoot.resolve(a.asset))
           model.saveTranscript(editedNote, a, text) { updated -> revision = updated.revision }
          } catch (e: CancellationException) { throw e }
          catch (e: Exception) { model.error = e.message ?: "转写失败，录音已保留" }
          finally { transcribing = null }
         }
        }) { Text(if (transcribing == a.asset) "正在转写…" else "离线转写") }
       }
       a.transcript?.let { Text(it, modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp), color = TextInk) }
      }
      else TextButton(enabled = !editorBusy, onClick={model.openAttachment(a)}) {LineIcon("file");Text("  "+a.name)}
     }
     if (model.recordingToken != null && !model.busy) Row {
      TextButton(onClick = { model.error = null; commitRecording() }) { Text("保存录音") }
      if (editId.isNotEmpty()) TextButton(onClick = { model.error = null; commitRecording(copy = true) }) { Text("另存为新便签") }
      TextButton(onClick = { model.discardRecording() }) { Text("丢弃此录音") }
     }
     Spacer(Modifier.height(32.dp))
    }
    HorizontalDivider(color=Edge)
    Row(Modifier.fillMaxWidth().navigationBarsPadding().horizontalScroll(rememberScrollState()).padding(horizontal=16.dp),verticalAlignment=Alignment.CenterVertically) {
     listOf("bold" to "加粗", "italic" to "斜体", "check" to "待办", "list" to "列表").forEach { (icon, label) ->
      IconButton(enabled = !recordingActive, onClick = { body = formatSelection(body, icon) }) { LineIcon(icon, label) }
     }
     IconButton(enabled=!editorBusy && editId.isNotEmpty() && !dirty,onClick={attachTo=model.notes.find {it.id==editId};picker.launch(arrayOf("*/*"))}) {LineIcon("image","添加图片或附件")}
     IconButton(enabled = model.recording || !editorBusy, onClick = {
      if (model.recording) model.stopRecording()
      else {
       audioPlayer.stop()
       if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) model.startRecording()
       else requestMic.launch(Manifest.permission.RECORD_AUDIO)
      }
     }) { LineIcon(if (model.recording) "stop" else "mic", if (model.recording) "停止录音" else "录音") }
     IconButton(enabled = !editorBusy && !dirty && editId.isNotEmpty(), onClick = {
      model.history(editId) { rows -> revisions = rows.filter { it.revision != revision } }
     }) { LineIcon("history", "历史版本") }
    }
   } else if(settings) AccountSettings(model)
   else if(page=="groups") {
    LazyColumn(modifier=Modifier.weight(1f),contentPadding=PaddingValues(start=20.dp,end=20.dp,top=26.dp,bottom=20.dp),verticalArrangement=Arrangement.spacedBy(14.dp)) {
     items(model.groups,key={it.id}) {g ->
      IndexPaper(Color(g.color or 0xFF000000),onClick={group=g.id;page="notes";trash=false}) {
       Row(Modifier.fillMaxWidth().padding(horizontal=18.dp,vertical=18.dp),verticalAlignment=Alignment.CenterVertically) {
        LineIcon("folder",color=Color(g.color or 0xFF000000),modifier=Modifier.size(21.dp));Text(g.name,fontSize=18.sp,modifier=Modifier.padding(start=16.dp).weight(1f))
        Text(model.notes.count {!it.deleted && it.group==g.id}.toString(),color=Muted);Spacer(Modifier.width(12.dp));LineIcon("right",modifier=Modifier.size(17.dp),color=Muted)
       }
      }
     }
    }
    Column(Modifier.padding(horizontal=20.dp)) {HorizontalDivider(color=Edge);TextButton(onClick={trash=true;group="";page="notes"},modifier=Modifier.fillMaxWidth().height(56.dp)) {LineIcon("trash",color=Muted,modifier=Modifier.size(18.dp));Text("  回收站",color=Muted,fontSize=14.sp);Spacer(Modifier.weight(1f));LineIcon("right",modifier=Modifier.size(16.dp),color=Muted)}}
   } else {
    PaperSearch(query) {query=it}
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal=20.dp,vertical=10.dp),horizontalArrangement=Arrangement.spacedBy(9.dp)) {
     PaperChip(if(trash) "回收站" else "全部",group.isEmpty()) {group=""}
     if(!trash) model.groups.forEach {g -> PaperChip(g.name,group==g.id) {group=g.id}}
     if(trash) TextButton(onClick={trash=false}) {Text("返回便签")}
    }
    val shown=model.notes.filter {it.deleted==trash && (trash || group.isEmpty() || it.group==group) && (it.title+"\n"+it.source).contains(query,ignoreCase=true)}
    LazyColumn(contentPadding=PaddingValues(start=20.dp,end=20.dp,top=4.dp,bottom=18.dp),verticalArrangement=Arrangement.spacedBy(12.dp)) {
     if(shown.isEmpty()) item {Column(Modifier.fillMaxWidth().padding(top=70.dp),horizontalAlignment=Alignment.CenterHorizontally) {LineIcon("note",color=Muted.copy(alpha=.55f),modifier=Modifier.size(28.dp));Text(if(trash) "回收站是空的" else if(query.isNotEmpty()) "没有找到相关便签" else "暂无便签",fontSize=16.sp,modifier=Modifier.padding(top=14.dp));Text(if(trash) "恢复的便签会回到原分组" else "点击 ＋ 新建便签",fontSize=13.sp,color=Muted,modifier=Modifier.padding(top=8.dp))}}
     items(shown,key={it.id}) {n ->
      val category=model.groups.find {it.id==n.group}
      var menu by remember {mutableStateOf(false)}
      IndexPaper(Color((category?.color ?: 0x245356) or 0xFF000000),onClick={if(!trash && !model.busy) open(n)}) {
       Column(Modifier.padding(start=17.dp,end=16.dp,top=13.dp,bottom=13.dp)) {
        Row(verticalAlignment=Alignment.CenterVertically) {
         Text(n.title.ifBlank {"无标题便签"},fontSize=18.sp,fontWeight=FontWeight.SemiBold,maxLines=2,overflow=TextOverflow.Ellipsis,modifier=Modifier.weight(1f))
         Box {IconButton(onClick={menu=true},modifier=Modifier.size(28.dp)) {LineIcon("more","便签操作",color=Muted,modifier=Modifier.size(18.dp))}
          DropdownMenu(menu,{menu=false}) {
           if(!trash) DropdownMenuItem(text={Text("添加附件")},onClick={menu=false;attachTo=n;picker.launch(arrayOf("*/*"))},enabled=!model.busy)
           DropdownMenuItem(text={Text(if(trash) "恢复便签" else "移入回收站")},onClick={menu=false;model.change(n,if(trash) "restore" else "delete")},enabled=!model.busy)
          }
         }
        }
        Spacer(Modifier.height(7.dp))
        val image=n.attachments.firstOrNull {it.isImage}
        Row(horizontalArrangement=Arrangement.spacedBy(14.dp),verticalAlignment=Alignment.CenterVertically) {
         NoteExcerpt(n.source,limit=if(image!=null) 1 else 4,modifier=Modifier.weight(1f))
         image?.let {a -> NoteThumbnail(a,root=model.assetRoot) {model.openAttachment(a)}}
        }
        n.attachments.filter {!it.isImage}.take(2).forEach {a ->
         Surface(onClick={model.openAttachment(a)},shape=RoundedCornerShape(5.dp),color=Quiet,border=BorderStroke(.5.dp,Edge),modifier=Modifier.padding(top=8.dp)) {
          Row(Modifier.fillMaxWidth().padding(9.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)) {LineIcon(if(a.isAudio) "play" else "file",modifier=Modifier.size(17.dp));Text(a.name,fontSize=12.sp,color=Muted,maxLines=1,overflow=TextOverflow.Ellipsis,modifier=Modifier.weight(1f));LineIcon("right",modifier=Modifier.size(13.dp),color=Muted)}
         }
        }
        Spacer(Modifier.height(12.dp))
        Row(verticalAlignment=Alignment.CenterVertically) {LineIcon("folder",color=Color((category?.color ?: 0x245356) or 0xFF000000),modifier=Modifier.size(13.dp));Text("  "+(category?.name ?: "默认"),fontSize=11.sp,color=Muted);Text("    |    ",fontSize=10.sp,color=Edge);Text(SimpleDateFormat("MM/dd",Locale.CHINA).format(Date(n.created)),fontSize=11.sp,color=Muted)}
        if(n.conflict!=null) {
         Text("存在同步冲突",color=MaterialTheme.colorScheme.error,fontSize=12.sp)
         val cloud=model.notes.find {it.id==n.conflict}
         if(cloud!=null) Row {listOf("local" to "保留本地","cloud" to "保留云端","both" to "保留两份").forEach {(choice,label) -> TextButton(enabled=!model.busy,onClick={model.send(JSONObject().put("op","resolve").put("id",n.id).put("revision",n.revision).put("cloud_revision",cloud.revision).put("choice",choice))}) {Text(label,fontSize=11.sp)}}}
        }
       }
      }
     }
    }
   }
  }
 }

 revisions?.let { entries ->
  if (selectedRevision == null) AlertDialog(onDismissRequest = { revisions = null }, title = { Text("历史版本") }, text = {
   if (entries.isEmpty()) Text("这台设备暂无更早的版本")
   else LazyColumn(Modifier.heightIn(max = 360.dp)) {
    items(entries, key = { it.revision }) { entry ->
     TextButton(onClick = { selectedRevision = entry }) {
      Text(SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.getDefault()).format(Date(entry.updated)) + "  " + entry.title.ifBlank { "无标题便签" })
     }
    }
   }
  }, confirmButton = { TextButton(onClick = { revisions = null }) { Text("关闭") } })
 }
 selectedRevision?.let { entry ->
  AlertDialog(onDismissRequest = { selectedRevision = null }, title = { Text(entry.title.ifBlank { "无标题便签" }) },
   text = { Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) { NoteExcerpt(entry.source, limit = Int.MAX_VALUE) } },
   confirmButton = { TextButton(enabled = !editorBusy, onClick = {
    model.notes.find { it.id == editId && it.revision == revision }?.let { note ->
     model.restoreHistory(note, entry) {
      model.lastSavedNote?.let { open(it) }
      selectedRevision = null; revisions = null
     }
    } ?: run { model.error = "便签已更新，请重新打开历史版本" }
   }) { Text("恢复此版本") } }, dismissButton = { TextButton(onClick = { selectedRevision = null }) { Text("返回") } })
 }

 if(showGroup) AlertDialog(onDismissRequest={showGroup=false},title={Text("新建分组")},text={Column(verticalArrangement=Arrangement.spacedBy(16.dp)) {
  OutlinedTextField(groupName,{groupName=it},label={Text("名称")},singleLine=true)
  Text("索引颜色",fontSize=13.sp,color=Muted)
  Row(horizontalArrangement=Arrangement.spacedBy(6.dp)) {listOf(0x245356L,0xC39B45L,0xB86543L,0x747E49L,0x8063A0L).forEach {color ->
   Surface(onClick={groupColor=color},shape=CircleShape,color=Color(color or 0xFF000000),modifier=Modifier.size(36.dp)) {Box(contentAlignment=Alignment.Center) {if(groupColor==color) LineIcon("tick",modifier=Modifier.size(16.dp),color=CardPaper)}}
  }}
 }},
  confirmButton={TextButton(enabled=groupName.isNotBlank() && !model.busy,onClick={model.send(JSONObject().put("op","group").put("name",groupName).put("color",groupColor)) {showGroup=false;groupName=""}}) {Text("创建")}},dismissButton={TextButton(onClick={showGroup=false}) {Text("取消")}})
 if(confirmLeave) AlertDialog(onDismissRequest={confirmLeave=false},title={Text("保存这张便签？")},text={Text("正文尚有修改，保存后返回列表。")},
  confirmButton={TextButton(enabled=!model.busy,onClick={confirmLeave=false;save()}) {Text("保存")}},
  dismissButton={Row {TextButton(onClick={confirmLeave=false}) {Text("继续编辑")};TextButton(onClick={confirmLeave=false;editing=false}) {Text("放弃修改")}}})
 model.error?.let { e -> AlertDialog(onDismissRequest={model.error=null},title={Text("操作未完成")},text={Text(e)},confirmButton={TextButton(onClick={model.error=null}) {Text("知道了")}}) }
 model.message?.let { message -> AlertDialog(onDismissRequest={model.message=null},text={Text(message)},confirmButton={TextButton(onClick={model.message=null}) {Text("好")}}) }
}

@Composable private fun AccountSettings(model: NotesModel) {
 var pendingUri by rememberSaveable { mutableStateOf<Uri?>(null) }
 var pendingExport by rememberSaveable { mutableStateOf(false) }
 val importZip = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri -> pendingExport = false; pendingUri = uri }
 val exportZip = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/zip")) { uri -> pendingExport = true; pendingUri = uri }
 LaunchedEffect(pendingUri, model.busy) {
  if (!model.busy) pendingUri?.let { uri -> pendingUri = null; model.transferZip(uri, export = pendingExport) }
 }
 var serverAddress by rememberSaveable { mutableStateOf("") }
 var username by rememberSaveable { mutableStateOf("") }
 val uriHandler = LocalUriHandler.current
 var password by remember { mutableStateOf("") }
 Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal=20.dp,vertical=16.dp),verticalArrangement=Arrangement.spacedBy(20.dp)) {
  Surface(shape=RoundedCornerShape(8.dp),color=CardPaper,border=BorderStroke(1.dp,Edge)) {
   Column(Modifier.padding(18.dp),verticalArrangement=Arrangement.spacedBy(16.dp)) {
    Row(verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(10.dp)) {LineIcon(if(model.synced) "cloud-check" else "cloud",modifier=Modifier.size(25.dp));Text("账号与同步",fontSize=19.sp,fontWeight=FontWeight.SemiBold)}
    Text(model.account?.let {"已登录 · $it"} ?: "填写自行部署的服务器地址后，即可登录并在手机和电脑之间同步便签。",color=Muted,fontSize=13.sp,lineHeight=21.sp)
    HorizontalDivider(color=Edge)
    if(model.account==null) {
     OutlinedTextField(
      serverAddress, { serverAddress = it }, label = { Text("同步服务器网址或 IP") },
      placeholder = { Text("https://sync.example.com") }, singleLine = true,
      shape = RoundedCornerShape(6.dp), modifier = Modifier.fillMaxWidth()
     )
     TextButton(onClick = {
      runCatching {
       uriHandler.openUri("https://github.com/Puring103/FlashNote#%E8%87%AA%E5%BB%BA%E5%90%8C%E6%AD%A5%E6%9C%8D%E5%8A%A1%E5%99%A8")
      }.onFailure { model.error = "无法打开服务器部署指南" }
     }) { Text("查看服务器部署指南") }
     OutlinedTextField(username,{username=it},label={Text("账号")},singleLine=true,shape=RoundedCornerShape(6.dp),modifier=Modifier.fillMaxWidth())
     OutlinedTextField(password,{password=it},label={Text("密码")},singleLine=true,visualTransformation=PasswordVisualTransformation(),shape=RoundedCornerShape(6.dp),modifier=Modifier.fillMaxWidth())
     Button(enabled=!model.busy && serverAddress.isNotBlank() && username.isNotBlank() && password.isNotBlank(),shape=RoundedCornerShape(6.dp),modifier=Modifier.fillMaxWidth().height(44.dp),onClick={model.send(JSONObject().put("op","login").put("url",serverAddress.trim()).put("username",username.trim()).put("password",password)) {password=""}}) {Text(if(model.busy) "正在登录…" else "登录")}
    } else {
     Text(when {model.syncing->"正在同步便签…";model.syncFailed->"同步未完成，请重试";model.synced->"便签已同步";else->"便签已保存在本机"},fontSize=13.sp,color=Muted)
     Button(enabled=!model.busy,shape=RoundedCornerShape(6.dp),modifier=Modifier.fillMaxWidth(),onClick={model.sync(notify = true)}) {Text("立即同步")}
     TextButton(enabled=!model.busy,onClick={model.send(JSONObject().put("op","logout"))}) {Text("退出登录")}
    }
   }
  }
  Surface(shape = RoundedCornerShape(8.dp), color = CardPaper, border = BorderStroke(1.dp, Edge)) {
   Column(Modifier.fillMaxWidth().padding(18.dp)) {
    Text("便签导入与导出", fontSize = 19.sp, fontWeight = FontWeight.SemiBold)
    Text("ZIP 含明文便签与附件。同 ID 的便签导入时保留为副本。", color = Muted, fontSize = 13.sp)
    Row {
     TextButton(enabled = !model.busy, onClick = { importZip.launch(arrayOf("application/zip", "application/x-zip-compressed", "application/octet-stream")) }) { Text("导入 ZIP") }
     TextButton(enabled = !model.busy, onClick = { exportZip.launch("FlashNote.zip") }) { Text("导出 ZIP") }
    }
   }
  }
  Text("索笺  /  Android",fontSize=12.sp,color=Muted)
  Text("便签保存在本机，卸载前请完成同步。",fontSize=12.sp,lineHeight=20.sp,color=Muted)
 }
}
