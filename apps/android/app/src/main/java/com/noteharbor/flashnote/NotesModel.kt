package com.noteharbor.flashnote

import android.app.Application
import android.net.Uri
import android.os.SystemClock
import androidx.compose.runtime.*
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.util.UUID
import kotlin.coroutines.cancellation.CancellationException
import java.util.concurrent.Executors

data class Attachment(val asset: String, val name: String, val audioBlock: String? = null,
 val transcript: String? = null, val durationMs: Long = 0) {
 val isImage: Boolean get()=name.substringAfterLast('.').lowercase() in listOf("jpg","jpeg","png","webp","gif")
 val isAudio: Boolean get()=name.substringAfterLast('.').lowercase() in listOf("wav","mp3","m4a","ogg","aac")
}
data class NoteRevision(val revision: String, val title: String, val source: String, val updated: Long)
data class Group(val id: String, val name: String, val color: Long = 0x245356)
data class Note(val id: String, val revision: String, val title: String, val group: String,
 val source: String, val deleted: Boolean, val created: Long, val conflict: String?, val attachments: List<Attachment> = emptyList())

class NotesModel(app: Application, preview: Boolean=false) : AndroidViewModel(app) {
 private val executor = Executors.newSingleThreadExecutor()
 private val worker = executor.asCoroutineDispatcher()
 private val root = app.filesDir.resolve(if(BuildConfig.DEBUG && preview) "design-preview-v2" else "vault").absolutePath
 private var storageRoot by mutableStateOf(root)
 var hasLocal by mutableStateOf(false); private set
 val assetRoot get()=java.io.File(storageRoot,"assets")
 var notes by mutableStateOf(emptyList<Note>()); private set
 var groups by mutableStateOf(emptyList<Group>()); private set
 var e2ee by mutableStateOf(false); private set
 var account by mutableStateOf<String?>(null); private set
 var busy by mutableStateOf(false); private set
 var syncing by mutableStateOf(false); private set
 var synced by mutableStateOf(false); private set
 var syncFailed by mutableStateOf(false); private set
 private var lastSyncAttempt: Long? = null
 private var syncPending = false
 private var foregroundSyncRequested = false
 var error by mutableStateOf<String?>(null)
 var lastSavedNote: Note? = null; private set
 var message by mutableStateOf<String?>(null)
 private var recorder: NoteRecording? = null
 var recording by mutableStateOf(false); private set
 var recordingToken by mutableStateOf<String?>(null); private set
 init { send(JSONObject().put("op","list"),prepare={if(BuildConfig.DEBUG && preview) DesignFixtures.seed(app,root)}) }
 fun foregroundEntered() { foregroundSyncRequested = true }

 fun sync(automatic: Boolean = false, notify: Boolean = false) {
  if (busy || account == null) return
  val elapsed = lastSyncAttempt?.let { SystemClock.elapsedRealtime() - it }
  val interval = when {
   foregroundSyncRequested -> 5_000L
   syncPending -> 10_000L
   else -> 60_000L
  }
  if (automatic && elapsed != null && elapsed < interval) return
  foregroundSyncRequested = false
  lastSyncAttempt = SystemClock.elapsedRealtime()
  send(JSONObject().put("op", "sync"), quiet = automatic) { if (notify) message = "同步完成" }
 }

 fun send(request: JSONObject, prepare: (() -> Unit)? = null,
  transferResult: (() -> Unit)? = null, cleanup: (() -> Unit)? = null,
  details: (Any?) -> Unit = {}, quiet: Boolean = false, done: () -> Unit = {}) {
  if (busy) return
  busy = true
  val operation = request.getString("op")
  syncing = operation == "sync"
  viewModelScope.launch {
   try {
    val data = withContext(worker) {
     try {
      prepare?.invoke()
      nativeRequest(request).also { transferResult?.invoke() }
     } finally { cleanup?.invoke() }
    }
    storageRoot=data.getString("storage_root")
    hasLocal=data.getBoolean("has_local")
    val ns = data.getJSONArray("notes")
    val changed = (0 until ns.length()).map { i ->
     val row=ns.getJSONObject(i); val n=row.getJSONObject("note")
     Note(n.getString("id"),n.getString("revision"),n.getString("title"),n.getString("group_id"),
      row.getString("source"), !n.isNull("deleted_at"),n.getLong("created_at"),
      if(n.isNull("conflict_of") || n.optBoolean("conflict_resolved")) null else n.getString("conflict_of"),
      n.getJSONArray("blocks").let { blocks -> (0 until blocks.length()).map { blocks.getJSONObject(it) }
       .filter { it.has("asset") && it.getString("asset").isNotEmpty() }
       .map {
        val audio = it.optString("kind") == "audio"
        Attachment(it.getString("asset"), it.optString("name", if (audio) "录音.wav" else "附件"),
         if (audio) it.getString("id") else null, if (it.isNull("text")) null else it.getString("text"),
         it.optLong("duration_ms"))
       } })
    }
    if (data.getBoolean("replace_notes")) {
     notes = changed.sortedByDescending { it.created }
    } else if (changed.isNotEmpty()) {
     val changesById = changed.associateBy { it.id }
     val currentIds = notes.mapTo(HashSet()) { it.id }
     notes = (notes.map { changesById[it.id] ?: it } + changed.filter { it.id !in currentIds })
      .sortedByDescending { it.created }
    }
    if (!data.isNull("saved_id")) lastSavedNote = changed.find { it.id == data.getString("saved_id") }
    val gs=data.getJSONArray("groups")
    groups=(0 until gs.length()).map { gs.getJSONObject(it) }.filter { !it.optBoolean("deleted") }.map { Group(it.getString("id"),it.getString("name"),it.optLong("color",0x245356)) }
    e2ee=data.optBoolean("e2ee")
    account=if(data.isNull("account")) null else data.getString("account")
    when(operation) {
     "sync" -> {
      val syncError = if (data.isNull("sync_error")) null else data.getString("sync_error")
      synced = syncError == null
      syncFailed = syncError != null
      syncPending = false
      if (syncError != null) {
       if (!quiet) error = syncError
       return@launch
      }
     }
     "login", "logout" -> { synced = false; syncFailed = false; lastSyncAttempt = null }
     "save", "attach", "group", "delete", "restore", "resolve", "recording", "transcript", "restore_history", "import_zip" -> {
      synced = false
      syncPending = true
     }
    }
    details(data.opt("details"))
    done()
   } catch(e: CancellationException) { throw e }
   catch(e: Exception) {
    if (syncing) { syncFailed = true; synced = false; syncPending = false }
    if (!quiet) error = e.message ?: "操作失败，草稿仍保留"
   }
   finally { busy=false;syncing=false }
  }
 }

 private fun nativeRequest(request: JSONObject): JSONObject {
  val response = JSONObject(Native.request(root, request.toString()))
  check(response.getBoolean("ok")) { response.optString("error", "操作失败") }
  return response.getJSONObject("data")
 }

 fun history(noteId: String, done: (List<NoteRevision>) -> Unit) {
  send(JSONObject().put("op", "history").put("id", noteId), details = { value ->
   val rows = value as org.json.JSONArray
   done(List(rows.length()) { index ->
    val row = rows.getJSONObject(index)
    NoteRevision(row.getString("revision"), row.getString("title"), row.getString("source"), row.getLong("updated"))
   })
  })
 }

 fun restoreHistory(note: Note, revision: NoteRevision, done: () -> Unit) = send(JSONObject()
  .put("op", "restore_history").put("id", note.id).put("revision", note.revision)
  .put("restore_revision", revision.revision), done = done)

 fun transferZip(uri: Uri, export: Boolean) {
  val token = UUID.randomUUID().toString()
  val file = transferFile(token)
  val resolver = getApplication<Application>().contentResolver
  send(JSONObject().put("op", if (export) "export_zip" else "import_zip").put("token", token),
   prepare = {
    file.parentFile?.mkdirs()
    if (!export) {
     checkNotNull(resolver.openInputStream(uri)) { "无法打开 ZIP" }.use { input ->
      file.outputStream().use { output ->
       val buffer = ByteArray(64 * 1024)
       var total = 0L
       while (true) {
        val size = input.read(buffer)
        if (size < 0) break
        total += size
        check(total <= 520L * 1024 * 1024) { "ZIP 超过 520 MB" }
        output.write(buffer, 0, size)
       }
      }
     }
    }
   }, transferResult = {
    if (export) checkNotNull(resolver.openOutputStream(uri, "wt")) { "无法写入 ZIP" }.use { output ->
     file.inputStream().use { it.copyTo(output) }
    }
   }, cleanup = { file.delete() }, details = { count ->
    message = if (export) "已导出 ZIP" else "已导入 $count 张便签"
   })
 }

 fun transferFile(token: String) = File(root, "imports/$token")

 fun startRecording() {
  if (recording || recordingToken != null || busy) return
  val token = UUID.randomUUID().toString()
  try {
   recorder = NoteRecording(getApplication(), transferFile(token)) { result ->
    viewModelScope.launch {
     recorder = null
     recording = false
     result.onSuccess { recordingToken = token }.onFailure { error = it.message ?: "录音失败" }
    }
   }
   recording = true
  } catch (e: Exception) { error = e.message ?: "无法开始录音" }
 }

 fun stopRecording() { recorder?.stop() }

 fun discardRecording() {
  recordingToken?.let { transferFile(it).delete() }
  recordingToken = null
 }

 fun saveRecording(noteId: String, revision: String, title: String, source: String, group: String,
  token: String, done: () -> Unit) = send(JSONObject().put("op", "recording")
  .put("id", noteId).put("revision", revision).put("title", title).put("source", source)
  .put("group", group).put("token", token), transferResult = { transferFile(token).delete() }) {
   recordingToken = null
   done()
  }

 fun saveTranscript(note: Note, item: Attachment, text: String, done: (Note) -> Unit) {
  send(JSONObject().put("op", "transcript").put("id", note.id).put("block", item.audioBlock).put("text", text)) {
   notes.find { it.id == note.id }?.let(done)
  }
 }
 fun save(note: Note?, title: String, source: String, group: String, done: () -> Unit) = send(JSONObject()
  .put("op","save").put("id",note?.id ?: "").put("revision",note?.revision ?: "")
  .put("title",title).put("source",source).put("group",group),done=done)
 fun change(note: Note, op: String, done: () -> Unit = {}) = send(JSONObject().put("op",op).put("id",note.id).put("revision",note.revision),done=done)
 fun attach(uri: android.net.Uri, note: Note) {
  val app=getApplication<Application>(); val token=java.util.UUID.randomUUID().toString()
  var name="附件"
  app.contentResolver.query(uri,null,null,null,null)?.use { cursor ->
   val i=cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
   if(cursor.moveToFirst() && i>=0) name=cursor.getString(i)
  }
  send(JSONObject().put("op","attach").put("id",note.id).put("revision",note.revision).put("token",token).put("name",name),prepare={
   val directory=java.io.File(storageRoot,"imports").apply {mkdirs()}; val file=java.io.File(directory,token)
   try {
    app.contentResolver.openInputStream(uri)!!.use { input -> file.outputStream().use { output ->
     val buffer=ByteArray(8192); var total=0L
     while(true) {val count=input.read(buffer);if(count<0) break;total+=count;check(total<=32L*1024*1024) {"附件超过 32MB"};output.write(buffer,0,count)}
    } }
   } catch(e: Exception) {file.delete();throw e}
  })
 }
 fun openAttachment(item: Attachment) {
  if(busy) return
  busy=true
  viewModelScope.launch {
   try {
    val data=withContext(worker) {JSONObject(Native.request(root,JSONObject().put("op","asset").put("asset",item.asset).put("name",item.name).toString()))}
    check(data.getBoolean("ok")) {data.optString("error")}
    val app=getApplication<Application>()
    val uri=androidx.core.content.FileProvider.getUriForFile(app,app.packageName+".files",java.io.File(data.getJSONObject("data").getString("file")))
    val ext=item.name.substringAfterLast('.',"").lowercase()
    val mime=android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
    val intent=android.content.Intent(android.content.Intent.ACTION_VIEW).setDataAndType(uri,mime).addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
    app.startActivity(intent)
   } catch(e: Exception) {error=e.message ?: "无法打开附件"}
   finally {busy=false}
  }
 }
 override fun onCleared() { stopRecording(); worker.close(); executor.shutdown(); super.onCleared() }
}
