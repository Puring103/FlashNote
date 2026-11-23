package com.noteharbor.flashnote

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.util.UUID

@Composable fun NoteThumbnail(attachment: Attachment, root: File, modifier: Modifier=Modifier.size(96.dp,72.dp), onClick: () -> Unit) {
 val context=LocalContext.current
 val bitmap by produceState<ImageBitmap?>(null,root,attachment.asset) {
  value=withContext(Dispatchers.IO) {
   runCatching {
    UUID.fromString(attachment.asset)
    val path=File(root,attachment.asset).absolutePath
    val bounds=BitmapFactory.Options().apply {inJustDecodeBounds=true}
    BitmapFactory.decodeFile(path,bounds)
    val options=BitmapFactory.Options().apply {
     inSampleSize=1
     while(bounds.outWidth/inSampleSize>512 || bounds.outHeight/inSampleSize>512) inSampleSize*=2
    }
    BitmapFactory.decodeFile(path,options)?.asImageBitmap()
   }.getOrNull()
  }
 }
 bitmap?.let {Image(it,attachment.name,contentScale=ContentScale.Crop,modifier=modifier.clip(RoundedCornerShape(6.dp)).clickable(onClick=onClick))}
}
